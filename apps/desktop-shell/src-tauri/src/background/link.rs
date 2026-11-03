//! The editor's one connection to the runtime. Commands and telemetry subscriptions share
//! it, each frame tagged with its request ID, so a slow command never holds up another
//! command or a stream and the editor needs one runtime connection slot.
use mixrelay_desktop_core::runtime_link::RouteTable;
use mixrelay_time::MonotonicClock;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mixrelay_control_protocol::TelemetryDecoder;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::control_channel::Mailbox;

/// Frames queued for the writer; a sender that waits longer is told the runtime stalled.
const OUTBOUND_FRAMES: usize = 32;

pub(super) enum Route {
    /// Every reply frame of one command, the last one terminal.
    Command(mpsc::UnboundedSender<Value>),
    /// Decoded windows of one subscription; the stream's own decoder holds its bindings.
    Telemetry {
        mailbox: Arc<Mailbox>,
        decoder: TelemetryDecoder,
    },
}

pub(super) struct Link {
    routes: Mutex<RouteTable<Route>>,
    clock: Arc<dyn MonotonicClock>,
    outbound: mpsc::Sender<Vec<u8>>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

impl Link {
    /// Starts the reader and writer for a verified stream.
    pub(super) fn start(
        stream: UnixStream,
        write_timeout: Duration,
        clock: Arc<dyn MonotonicClock>,
    ) -> Arc<Self> {
        let (read, write) = stream.into_split();
        let (outbound, frames) = mpsc::channel(OUTBOUND_FRAMES);
        let link = Arc::new(Self {
            routes: Mutex::new(RouteTable::new(clock.now())),
            clock,
            outbound,
            tasks: Mutex::default(),
        });
        let reader = tokio::spawn(Arc::clone(&link).read(read));
        let writer = tokio::spawn(Arc::clone(&link).write(write, frames, write_timeout));
        *link.tasks.lock().unwrap_or_else(PoisonError::into_inner) =
            vec![reader.abort_handle(), writer.abort_handle()];
        // A peer that left at once may have closed the link before the tasks were
        // registered; closing again stops them now that they are.
        if link.routes().closed() {
            link.close();
        }
        link
    }

    fn routes(&self) -> std::sync::MutexGuard<'_, RouteTable<Route>> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a route unless the link closed or idled past `reuse`; a link idle that
    /// long may be one the runtime is about to close, so it is retired instead.
    pub(super) fn register(&self, id: u64, route: Route, reuse: Duration) -> bool {
        let mut routes = self.routes();
        if routes.register(id, route, self.clock.now(), reuse).is_ok() {
            true
        } else {
            drop(routes);
            self.close();
            false
        }
    }

    pub(super) fn remove(&self, id: u64) {
        self.routes().remove(id, self.clock.now());
    }

    /// Queues a frame; `false` means the writer never took it, so it was never sent.
    /// The caller decides what a refused frame means for its route and this link.
    pub(super) async fn send(&self, frame: Vec<u8>, wait: Duration) -> bool {
        matches!(
            tokio::time::timeout(wait, self.outbound.send(frame)).await,
            Ok(Ok(()))
        )
    }

    /// Ends a subscription: forgets its route and tells the runtime. A full queue sends
    /// the cancel once it drains; if it cannot, the link closes, which the runtime also
    /// treats as the end of every subscription on it.
    pub(super) fn cancel(self: &Arc<Self>, id: u64, wait: Duration) {
        self.remove(id);
        let frame = json!({"id": 0, "command": {"type": "cancel", "subscription": id}});
        let frame = format!("{frame}\n").into_bytes();
        let Err(mpsc::error::TrySendError::Full(frame)) = self.outbound.try_send(frame) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return self.close();
        };
        let link = Arc::clone(self);
        runtime.spawn(async move {
            if !link.send(frame, wait).await {
                link.close();
            }
        });
    }

    /// Fails every open route and stops both tasks. Commands learn the runtime may have
    /// received them; subscriptions end.
    pub(super) fn close(&self) {
        let live = {
            let mut routes = self.routes();
            routes.close()
        };
        for route in live.into_values() {
            if let Route::Telemetry { mailbox, .. } = route {
                mailbox.end("runtime connection closed");
            }
        }
        for task in self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }

    async fn write(
        self: Arc<Self>,
        mut write: OwnedWriteHalf,
        mut frames: mpsc::Receiver<Vec<u8>>,
        timeout: Duration,
    ) {
        while let Some(frame) = frames.recv().await {
            if !matches!(
                tokio::time::timeout(timeout, write.write_all(&frame)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        self.close();
    }

    async fn read(self: Arc<Self>, read: OwnedReadHalf) {
        let limit = crate::control_channel::RESPONSE_LIMIT;
        let mut reader = BufReader::new(read);
        loop {
            let mut line = Vec::new();
            let complete = (&mut reader)
                .take(limit as u64 + 1)
                .read_until(b'\n', &mut line)
                .await
                .is_ok_and(|_| line.len() <= limit && line.last() == Some(&b'\n'));
            let Some(frame) = complete
                .then(|| serde_json::from_slice::<Value>(&line).ok())
                .flatten()
            else {
                break;
            };
            self.route(frame);
        }
        self.close();
    }

    /// Delivers one frame. Frames for routes that already ended are dropped.
    fn route(&self, mut frame: Value) {
        let Some(id) = frame["id"].as_u64() else {
            return;
        };
        let mut routes = self.routes();
        let ended = match routes.get_mut(id) {
            None => return,
            Some(Route::Command(replies)) => {
                let terminal = frame["status"] != 102;
                let _ = replies.send(frame);
                terminal
            }
            Some(Route::Telemetry { mailbox, decoder }) => {
                if frame["status"] == 206 {
                    match decoder.json(frame["body"].take()) {
                        Ok(window) => {
                            mailbox.deliver(window);
                            false
                        }
                        Err(reason) => {
                            mailbox.end(&reason.to_string());
                            true
                        }
                    }
                } else {
                    let reason = frame["body"]["parameters"]["reason"].as_str();
                    mailbox.end(reason.unwrap_or("meter push unavailable"));
                    true
                }
            }
        };
        if ended {
            routes.remove(id, self.clock.now());
        }
    }
}

#[cfg(test)]
mod tests;
