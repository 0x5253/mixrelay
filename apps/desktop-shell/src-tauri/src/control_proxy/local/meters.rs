//! The runtime's single engine meter stream, fanned out to every subscribed editor.
//! Each editor has its own mailbox, so a slow editor never delays the engine stream or
//! another editor; it keeps the latest window with the interval maximum.
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use mixrelay_control_protocol::ControlTelemetry;
use tokio::sync::watch;

use crate::{
    control_channel::{Mailbox, SILENCE_LIMIT},
    control_proxy::telemetry::EventParser,
};

/// Bounds opening the engine stream, which the shared HTTP client does not.
pub(crate) const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Where engine meter windows come from: the engine's server-sent event stream.
pub(super) trait TelemetrySource: Send + Sync + 'static {
    type Chunks: TelemetryChunks;
    fn open(&self) -> impl Future<Output = Result<Self::Chunks, String>> + Send;
    fn observe(
        &self,
        _window: &mut ControlTelemetry,
    ) -> impl Future<Output = Result<(), String>> + Send {
        async { Ok(()) }
    }
}

/// The body of an open meter stream.
pub(super) trait TelemetryChunks: Send + 'static {
    /// The next body chunk, `None` at the end of the stream.
    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send;
    fn incarnation(&self) -> Option<String> {
        None
    }
}

impl TelemetrySource for super::LocalControl {
    type Chunks = reqwest::Response;

    fn open(&self) -> impl Future<Output = Result<Self::Chunks, String>> + Send {
        self.meter_stream()
    }

    fn observe(
        &self,
        window: &mut ControlTelemetry,
    ) -> impl Future<Output = Result<(), String>> + Send {
        self.observe_telemetry(window)
    }
}

impl TelemetrySource for Arc<super::LocalControl> {
    type Chunks = reqwest::Response;
    fn open(&self) -> impl Future<Output = Result<Self::Chunks, String>> + Send {
        self.meter_stream()
    }
    fn observe(
        &self,
        window: &mut ControlTelemetry,
    ) -> impl Future<Output = Result<(), String>> + Send {
        self.observe_telemetry(window)
    }
}

impl TelemetryChunks for reqwest::Response {
    fn incarnation(&self) -> Option<String> {
        self.headers()
            .get("X-MixRelay-Engine")
            .and_then(|header| header.to_str().ok())
            .map(str::to_owned)
    }
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.chunk()
            .await
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .map_err(|error| error.to_string())
    }
}

pub(super) struct TelemetryHub<S> {
    source: Arc<S>,
    stop: watch::Receiver<bool>,
    readers: Mutex<Readers>,
}

#[derive(Default)]
struct Readers {
    mailboxes: Vec<Weak<Mailbox>>,
    /// The running upstream's generation; `None` when no upstream runs.
    upstream: Option<u64>,
    generations: u64,
    latest: Option<ControlTelemetry>,
}

impl<S: TelemetrySource> TelemetryHub<S> {
    pub(super) fn new(source: Arc<S>, stop: watch::Receiver<bool>) -> Arc<Self> {
        Arc::new(Self {
            source,
            stop,
            readers: Mutex::default(),
        })
    }

    fn readers(&self) -> std::sync::MutexGuard<'_, Readers> {
        self.readers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a mailbox, opening the engine stream if no other editor holds it open.
    /// Dropping the mailbox unsubscribes; the stream closes with its last reader.
    pub(super) fn subscribe(self: &Arc<Self>) -> Arc<Mailbox> {
        let mailbox = Arc::new(Mailbox::default());
        let mut readers = self.readers();
        if let Some(latest) = &readers.latest {
            mailbox.deliver(latest.clone());
        }
        readers.mailboxes.push(Arc::downgrade(&mailbox));
        if readers.upstream.is_none() {
            readers.generations += 1;
            let generation = readers.generations;
            readers.upstream = Some(generation);
            tokio::spawn(Arc::clone(self).upstream(generation));
        }
        mailbox
    }

    async fn upstream(self: Arc<Self>, generation: u64) {
        let mut stop = self.stop.clone();
        let reason = tokio::select! {
            biased;
            _ = stop.changed() => "runtime stopped".to_owned(),
            reason = self.relay() => reason,
        };
        let mut readers = self.readers();
        // An upstream that already gave up ownership must not end its successor's readers.
        if readers.upstream == Some(generation) {
            readers.upstream = None;
            readers.latest = None;
            for mailbox in readers
                .mailboxes
                .drain(..)
                .filter_map(|weak| weak.upgrade())
            {
                mailbox.end(&reason);
            }
        }
    }

    /// Publishes windows until the engine stream fails, falls silent, or loses its readers.
    async fn relay(&self) -> String {
        let mut chunks = match tokio::time::timeout(OPEN_TIMEOUT, self.source.open()).await {
            Ok(Ok(chunks)) => chunks,
            Ok(Err(reason)) => return reason,
            Err(_) => return "telemetry stream did not open".to_owned(),
        };
        let incarnation = chunks.incarnation();
        let mut parser = EventParser::default();
        // The engine sends meter bindings only when they change on this stream.
        let mut decoder = mixrelay_control_protocol::TelemetryDecoder::default();
        loop {
            let chunk = match tokio::time::timeout(SILENCE_LIMIT, chunks.next_chunk()).await {
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => return "telemetry stream ended".to_owned(),
                Ok(Err(reason)) => return reason,
                Err(_) => return "telemetry stream stalled".to_owned(),
            };
            let events = match parser.push(&chunk) {
                Ok(events) => events,
                Err(reason) => return reason,
            };
            for event in events {
                let window = serde_json::from_str(&event)
                    .map_err(|error| error.to_string())
                    .and_then(|value| decoder.json(value).map_err(|error| error.to_string()));
                // A lost binding table would misplace every later window.
                let mut window = match window {
                    Ok(window) => window,
                    Err(reason) => return reason,
                };
                if incarnation
                    .as_ref()
                    .is_some_and(|engine| engine != &window.engine_incarnation)
                {
                    return "telemetry engine identity changed".to_owned();
                }
                match tokio::time::timeout(OPEN_TIMEOUT, self.source.observe(&mut window)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(reason)) => return reason,
                    Err(_) => return "control observation stalled".to_owned(),
                }
                if !self.publish(&window) {
                    return "no meter readers".to_owned();
                }
            }
        }
    }

    /// Delivers a window to every live mailbox. With none left, gives up the upstream
    /// under the same lock a new reader would take, so that reader starts a fresh one.
    /// Only the running upstream publishes: ownership changes only once it gave up.
    fn publish(&self, window: &ControlTelemetry) -> bool {
        let mut readers = self.readers();
        let state = window.state.clone().or_else(|| {
            readers
                .latest
                .as_ref()
                .filter(|old| old.engine_incarnation == window.engine_incarnation)
                .and_then(|old| old.state.clone())
        });
        readers.latest = Some(ControlTelemetry {
            state,
            ..window.clone()
        });
        readers.mailboxes.retain(|weak| match weak.upgrade() {
            Some(mailbox) => {
                mailbox.deliver(window.clone());
                true
            }
            None => false,
        });
        if readers.mailboxes.is_empty() {
            readers.upstream = None;
            readers.latest = None;
            return false;
        }
        true
    }
}

/// Moves one subscription's windows from the shared engine stream to its connection,
/// until the engine stream ends (ending the subscription with the reason) or the
/// connection cancels it.
pub(super) async fn pump<S: TelemetrySource>(
    id: u64,
    hub: Arc<TelemetryHub<S>>,
    outbound: Arc<super::outbound::Outbound>,
) {
    let mailbox = hub.subscribe();
    loop {
        match mailbox.take().await {
            Ok(window) => outbound.window(id, window),
            Err(reason) => return outbound.end(id, &reason),
        }
    }
}

#[cfg(test)]
mod tests;
