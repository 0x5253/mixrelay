//! One multiplexed editor connection owns command delivery and telemetry subscriptions.
use super::connection_failure::ConnectionFailure;
#[cfg(test)]
use super::startup;
use super::{control_directory, link};
use mixrelay_control_protocol::ControlTelemetry;
use mixrelay_desktop_application::runtime_link::{CommandFrame, command_frame};
use serde_json::{Value, json};
use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tauri::Manager as _;
use tokio::net::UnixStream;

pub(super) const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(3);
/// Covers the runtime opening the engine stream (`local::meters::OPEN_TIMEOUT`) and its
/// first window.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(8);
/// How long an idle link stays reusable: well inside the controller's idle timeout, so a
/// reused link is never one it is closing for idleness. A controller exiting between the
/// check and the write fails that request as an exit mid-request would; it is never
/// replayed.
const REUSE_WINDOW: Duration = Duration::from_secs(10);
const _: () = assert!(
    REUSE_WINDOW.as_secs() * 2 < crate::control_proxy::local::socket::IDLE_TIMEOUT.as_secs()
);

#[derive(Clone)]
pub(crate) struct Connection {
    pub(super) connect_gate: Arc<tokio::sync::Mutex<()>>,
    clock: Arc<dyn mixrelay_time::MonotonicClock>,
    path: PathBuf,
    uid: u32,
    next_request: Arc<AtomicU64>,
    link: Arc<tokio::sync::Mutex<Option<Arc<link::Link>>>>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Connection")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Connection {
    #[cfg(test)]
    pub(super) fn for_socket_fixture(path: PathBuf, uid: u32) -> Self {
        Self {
            connect_gate: Arc::default(),
            clock: Arc::new(mixrelay_time::ManualClock::default()),
            path,
            uid,
            next_request: Arc::new(AtomicU64::new(1)),
            link: Arc::default(),
        }
    }

    pub(crate) fn for_app(app: &tauri::AppHandle) -> Result<Self, ConnectionFailure> {
        let uid = std::fs::metadata(
            app.path()
                .home_dir()
                .map_err(ConnectionFailure::HomeDirectory)?,
        )
        .map_err(ConnectionFailure::HomeMetadata)?
        .uid();
        let directory = control_directory(app).map_err(ConnectionFailure::ControlDirectory)?;
        Ok(Self {
            connect_gate: Arc::default(),
            clock: Arc::new(mixrelay_time::StdClock::new()),
            path: directory.join("control.sock"),
            uid,
            next_request: Arc::new(AtomicU64::new(1)),
            link: Arc::default(),
        })
    }

    pub(crate) async fn invoke(&self, name: &str, arguments: Value) -> Result<Value, Value> {
        self.exchange(name, arguments)
            .await
            .map_err(|error| Value::String(error.to_string()))?
    }

    async fn connect(&self) -> Result<UnixStream, ConnectionFailure> {
        let stream = tokio::time::timeout(EXCHANGE_TIMEOUT, UnixStream::connect(&self.path))
            .await
            .map_err(|_| ConnectionFailure::ConnectTimeout)?
            .map_err(ConnectionFailure::Connect)?;
        let peer = stream
            .peer_cred()
            .map_err(ConnectionFailure::PeerCredentials)?;
        if peer.uid() != self.uid {
            return Err(ConnectionFailure::PeerMismatch);
        }
        Ok(stream)
    }

    /// Registers a route on the current link, opening a link when there is none or the
    /// current one can no longer take new work.
    async fn open<T>(
        &self,
        id: u64,
        route: &impl Fn() -> (link::Route, T),
    ) -> Result<(Arc<link::Link>, T), ConnectionFailure> {
        let mut current = self.link.lock().await;
        if let Some(link) = current.as_ref() {
            let (route, state) = route();
            if link.register(id, route, REUSE_WINDOW) {
                return Ok((Arc::clone(link), state));
            }
        }
        let link = link::Link::start(
            self.connect().await?,
            EXCHANGE_TIMEOUT,
            Arc::clone(&self.clock),
        );
        let (route, state) = route();
        if !link.register(id, route, REUSE_WINDOW) {
            return Err(ConnectionFailure::ClosedBeforeAdmission);
        }
        *current = Some(Arc::clone(&link));
        Ok((link, state))
    }

    /// Registers a route and queues its request. A request the runtime cannot have
    /// received is queued once more on a fresh link; nothing else is ever replayed.
    async fn start<T>(
        &self,
        id: u64,
        frame: &[u8],
        route: impl Fn() -> (link::Route, T),
    ) -> Result<(Arc<link::Link>, T), ConnectionFailure> {
        if frame.len() as u64 > crate::control_channel::FRAME_LIMIT {
            return Err(ConnectionFailure::RequestTooLarge);
        }
        for _ in 0..2 {
            let (link, state) = self.open(id, &route).await?;
            if link.send(frame.to_vec(), EXCHANGE_TIMEOUT).await {
                return Ok((link, state));
            }
            // Forget the route first: closing must not end what the retry registers anew.
            link.remove(id);
            link.close();
        }
        Err(ConnectionFailure::SendFailed)
    }

    /// Subscribes to pushed telemetry on the shared link. Succeeds only once the first
    /// window arrives, so an engine or runtime without push is an error the caller can
    /// fall back from, not a stream that ends at once.
    pub(crate) async fn subscribe_telemetry(
        &self,
    ) -> Result<StartedTelemetrySubscription, ConnectionFailure> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let frame = format!(
            "{}\n",
            json!({"id": id, "command": {"type": "subscribe_telemetry"}})
        );
        let (link, mailbox) = self
            .start(id, frame.as_bytes(), || {
                let mailbox = Arc::new(crate::control_channel::Mailbox::default());
                let route = link::Route::Telemetry {
                    mailbox: Arc::clone(&mailbox),
                    decoder: mixrelay_control_protocol::TelemetryDecoder::default(),
                };
                (route, mailbox)
            })
            .await?;
        let subscription = TelemetrySubscription { id, link, mailbox };
        // The runtime may first open the engine stream (bounded by its own timeout).
        match tokio::time::timeout(SUBSCRIBE_TIMEOUT, subscription.mailbox.take()).await {
            Ok(Ok(window)) => Ok(StartedTelemetrySubscription {
                first: Some(window),
                stream: subscription,
            }),
            Ok(Err(reason)) => Err(ConnectionFailure::SubscriptionRejected(reason)),
            Err(_) => Err(ConnectionFailure::SubscriptionTimeout),
        }
    }

    pub(super) async fn exchange(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<Result<Value, Value>, ConnectionFailure> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let mut frame =
            json!({"id":id,"command":{"type":"invoke","name":name,"arguments":arguments}})
                .to_string()
                .into_bytes();
        frame.push(b'\n');
        let (replies, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (link, ()) = self
            .start(id, &frame, || (link::Route::Command(replies.clone()), ()))
            .await?;
        // The route holds the only sender: a closed link ends `received`.
        drop(replies);
        loop {
            let reply = match tokio::time::timeout(EXCHANGE_TIMEOUT, received.recv()).await {
                Ok(Some(reply)) => reply,
                Ok(None) => return Err(ConnectionFailure::DisconnectedAfterAdmission),
                Err(_) => {
                    link.remove(id);
                    return Err(ConnectionFailure::ResponseTimeout);
                }
            };
            match command_frame(reply) {
                CommandFrame::Heartbeat => {}
                CommandFrame::Complete(value) => return Ok(value),
                CommandFrame::ControllerUnavailable => {
                    return Err(ConnectionFailure::ControllerUnavailable);
                }
                CommandFrame::InvalidResponse => return Err(ConnectionFailure::InvalidResponse),
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn running(&self) -> bool {
        self.invoke("hello", Value::Null)
            .await
            .is_ok_and(|hello| startup::compatible(&hello))
    }
}

/// Pushed telemetry on the shared runtime link; dropping it cancels the subscription.
pub(crate) struct TelemetrySubscription {
    id: u64,
    link: Arc<link::Link>,
    mailbox: Arc<crate::control_channel::Mailbox>,
}

impl Drop for TelemetrySubscription {
    fn drop(&mut self) {
        self.link.cancel(self.id, EXCHANGE_TIMEOUT);
    }
}

/// A subscription whose first window has been read but not yet returned.
pub(crate) struct StartedTelemetrySubscription {
    first: Option<ControlTelemetry>,
    stream: TelemetrySubscription,
}

impl StartedTelemetrySubscription {
    /// The next window, or `None` once the runtime ends the stream, the link breaks, or it
    /// falls silent although the engine publishes continuously.
    pub(crate) async fn next(&mut self) -> Option<ControlTelemetry> {
        if let Some(window) = self.first.take() {
            return Some(window);
        }
        tokio::time::timeout(
            crate::control_channel::SILENCE_LIMIT,
            self.stream.mailbox.take(),
        )
        .await
        .ok()?
        .ok()
    }
}

#[cfg(test)]
mod tests;
