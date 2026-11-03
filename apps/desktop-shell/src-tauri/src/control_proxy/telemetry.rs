//! Pushed engine telemetry: the runtime reads the engine's server-sent events once and
//! relays each window to every subscribed editor, which forwards it to the webview over a
//! credited channel. No hop queues windows: a slow reader keeps one pending window that
//! newer windows fold into (latest state, interval maximum).
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::control_channel::Mailbox;
use mixrelay_control_protocol::{ControlTelemetry, TelemetryEncoder};
use tauri::Manager as _;
use tokio::sync::{Semaphore, watch};

/// Windows the webview may hold unacknowledged.
const CREDITS: usize = 2;
/// A webview that returns no credit for this long has stopped consuming.
const CREDIT_LIMIT: Duration = Duration::from_secs(10);
// A hidden editor needs state/liveness, while its meters are not displayed. Stay
// below the two-second UI watchdog; restoring visibility bypasses this interval.
const HIDDEN_INTERVAL: Duration = Duration::from_secs(1);

/// The editor's live meter subscriptions. Each removes itself when it ends.
#[derive(Default)]
pub(crate) struct TelemetrySubscriptions {
    next: AtomicU64,
    live: Mutex<HashMap<u64, Subscriber>>,
}

struct Subscriber {
    task: tauri::async_runtime::JoinHandle<()>,
    credit: Arc<Semaphore>,
    visible: watch::Sender<bool>,
}

impl TelemetrySubscriptions {
    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Subscriber>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Starts forwarding pushed meter windows to `channel` and returns the subscription id.
/// The webview returns one credit per window; after the last window the channel receives
/// `{"ended": true}`, so the UI can resubscribe.
///
/// # Errors
/// Fails when the runtime or engine cannot deliver a first window; the UI then polls.
#[tauri::command]
pub(crate) async fn control_subscribe_telemetry(
    app: tauri::AppHandle,
    channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
) -> Result<u64, String> {
    let connection = app
        .try_state::<crate::background::Connection>()
        .ok_or("meter push needs the background runtime")?
        .inner()
        .clone();
    let mut subscription = connection
        .subscribe_telemetry()
        .await
        .map_err(|error| error.to_string())?;
    let subscriptions = app.state::<TelemetrySubscriptions>();
    let id = subscriptions.next.fetch_add(1, Ordering::Relaxed) + 1;
    let credit = Arc::new(Semaphore::new(CREDITS));
    let (visible, mut visibility) = watch::channel(true);
    // Spawn under the lock so the task's own removal always follows its insertion.
    let mut live = subscriptions.live();
    let task = tauri::async_runtime::spawn({
        let app = app.clone();
        let credit = Arc::clone(&credit);
        async move {
            // This subscriber's encoder: bindings follow only when they change.
            let mut encoder = TelemetryEncoder::default();
            forward(
                &mut subscription,
                |delivery| {
                    channel
                        .send(webview_message(&mut encoder, &delivery))
                        .is_ok()
                },
                &credit,
                &mut visibility,
            )
            .await;
            app.state::<TelemetrySubscriptions>().live().remove(&id);
        }
    });
    live.insert(
        id,
        Subscriber {
            task,
            credit,
            visible,
        },
    );
    Ok(id)
}

/// What one webview subscriber receives, in order.
#[derive(Debug, PartialEq)]
pub(crate) enum Delivery {
    Window(Box<ControlTelemetry>),
    /// The stream ended; the UI resubscribes or falls back to polling.
    Ended,
}

/// The same credited channel carries binary telemetry frames and a terminal JSON marker.
fn webview_message(
    encoder: &mut TelemetryEncoder,
    delivery: &Delivery,
) -> tauri::ipc::InvokeResponseBody {
    match delivery {
        Delivery::Window(window) => tauri::ipc::InvokeResponseBody::Raw(encoder.raw(window)),
        Delivery::Ended => tauri::ipc::InvokeResponseBody::Json(r#"{"ended":true}"#.to_owned()),
    }
}

/// Returns a credit after the webview consumed a window.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) fn control_telemetry_credit(
    subscriptions: tauri::State<'_, TelemetrySubscriptions>,
    id: u64,
) {
    if let Some(subscriber) = subscriptions.live().get(&id)
        && subscriber.credit.available_permits() < CREDITS
    {
        subscriber.credit.add_permits(1);
    }
}

/// Changes only this editor's delivery cadence; the engine and shared reader continue.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) fn control_telemetry_visibility(
    subscriptions: tauri::State<'_, TelemetrySubscriptions>,
    id: u64,
    visible: bool,
) {
    if let Some(subscriber) = subscriptions.live().get(&id) {
        subscriber.visible.send_replace(visible);
    }
}

/// Ends a subscription; its runtime connection closes and the runtime drops its mailbox.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) fn control_unsubscribe_telemetry(
    subscriptions: tauri::State<'_, TelemetrySubscriptions>,
    id: u64,
) {
    if let Some(subscriber) = subscriptions.live().remove(&id) {
        subscriber.task.abort();
    }
}

/// A subscription's windows, `None` once it ended.
pub(crate) trait Windows: Send {
    fn next(&mut self) -> impl std::future::Future<Output = Option<ControlTelemetry>> + Send;
}

impl Windows for crate::background::StartedTelemetrySubscription {
    fn next(&mut self) -> impl std::future::Future<Output = Option<ControlTelemetry>> + Send {
        Self::next(self)
    }
}

/// Reads windows into a mailbox while sending them as credit allows, so a stalled webview
/// never accumulates messages. Ends with `{"ended": true}` (without credit when the
/// webview stopped returning it) unless the channel is gone.
pub(crate) async fn forward(
    windows: &mut impl Windows,
    mut send: impl FnMut(Delivery) -> bool,
    credit: &Semaphore,
    visibility: &mut watch::Receiver<bool>,
) {
    let mailbox = Mailbox::default();
    let receive = async {
        while let Some(window) = windows.next().await {
            mailbox.deliver(window);
        }
        mailbox.end("telemetry stream ended");
        std::future::pending::<()>().await;
    };
    let deliver = async {
        let mut next_hidden = None;
        loop {
            let Ok(Ok(permit)) = tokio::time::timeout(CREDIT_LIMIT, credit.acquire()).await else {
                // Best effort: a webview that resumes learns to resubscribe.
                send(Delivery::Ended);
                return;
            };
            let visible = *visibility.borrow_and_update();
            if !visible && let Some(deadline) = next_hidden {
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {},
                    changed = visibility.changed() => {
                        if changed.is_err() {
                            send(Delivery::Ended);
                            return;
                        }
                        continue;
                    }
                }
            }
            let Ok(window) = mailbox.take().await else {
                send(Delivery::Ended);
                return;
            };
            permit.forget();
            next_hidden =
                (!*visibility.borrow()).then(|| tokio::time::Instant::now() + HIDDEN_INTERVAL);
            if !send(Delivery::Window(Box::new(window))) {
                return;
            }
        }
    };
    tokio::select! {
        () = receive => {}
        () = deliver => {}
    }
}

/// Largest event the parser accepts; a longer one is malformed.
const EVENT_LIMIT: usize = 4 * 1024 * 1024;

/// Splits server-sent events out of response chunks. Events end with a blank line (LF, as
/// the engine's server writes them).
#[derive(Debug, Default)]
pub(crate) struct EventParser {
    buffer: Vec<u8>,
    /// Bytes already searched for a terminator, so a long event is scanned once.
    scanned: usize,
}

impl EventParser {
    /// Appends a chunk and returns the `data` payload of every event it completes.
    /// Comment lines (keep-alives) and fields other than `data` are ignored.
    ///
    /// # Errors
    /// Returns an error, and discards buffered data, when any event exceeds the limit.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut start = 0;
        let mut search = self.scanned.saturating_sub(1).max(start);
        while let Some(offset) = self.buffer[search..]
            .windows(2)
            .position(|pair| pair == b"\n\n")
        {
            let end = search + offset + 2;
            if end - start > EVENT_LIMIT {
                return Err(self.oversized());
            }
            let text = String::from_utf8_lossy(&self.buffer[start..end]);
            let data: Vec<&str> = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
                .collect();
            if !data.is_empty() {
                events.push(data.join("\n"));
            }
            start = end;
            search = end;
        }
        self.buffer.drain(..start);
        if self.buffer.len() > EVENT_LIMIT {
            return Err(self.oversized());
        }
        self.scanned = self.buffer.len();
        Ok(events)
    }

    fn oversized(&mut self) -> String {
        self.buffer.clear();
        self.scanned = 0;
        "telemetry event exceeds its limit".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_messages_pack_meters_as_raw_bytes_send_bindings_once_and_end_as_json() {
        let mut encoder = TelemetryEncoder::default();
        let metadata = |body: tauri::ipc::InvokeResponseBody| {
            let tauri::ipc::InvokeResponseBody::Raw(bytes) = body else {
                panic!("meter samples must be raw");
            };
            let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            assert_eq!(bytes[(length + 7) & !3..], 0.5_f32.to_le_bytes());
            serde_json::from_slice::<serde_json::Value>(&bytes[4..4 + length]).unwrap()
        };
        let first = metadata(webview_message(
            &mut encoder,
            &Delivery::Window(Box::new(window(1, 0.5))),
        ));
        assert_eq!(first["meters"]["bindings"][0]["channels"], 1);
        assert!(first["meters"].get("nodes").is_none());
        let second = metadata(webview_message(
            &mut encoder,
            &Delivery::Window(Box::new(window(2, 0.5))),
        ));
        assert!(second["meters"].get("bindings").is_none());
        assert!(matches!(
            webview_message(&mut encoder, &Delivery::Ended),
            tauri::ipc::InvokeResponseBody::Json(_)
        ));
    }

    fn window(sequence: u64, peak: f32) -> ControlTelemetry {
        ControlTelemetry {
            engine_incarnation: "a".into(),
            sequence,
            meters: mixrelay_control_protocol::MeterSnapshot {
                protocol_version: 1,
                revision: 1,
                nodes: vec![mixrelay_control_protocol::NodeMeterSnapshot {
                    node_id: "n".into(),
                    port_id: "out".into(),
                    peaks: vec![peak],
                    dynamics: None,
                }],
            },
            state: Some(mixrelay_control_protocol::ControlStateSnapshot {
                generation: sequence,
                status: mixrelay_control_protocol::AppStatus::default(),
                graph: mixrelay_control_protocol::GraphRuntimeStatus {
                    protocol_version: 1,
                    active_revision: sequence,
                    submitted_revision: sequence,
                    pending_reason: None,
                },
                controls: None,
                editors: Vec::new(),
            }),
        }
    }

    #[test]
    fn events_split_across_chunks_yield_their_data_and_skip_keep_alives() {
        let mut parser = EventParser::default();
        assert!(
            parser
                .push(b"event: meters\ndata: {\"a\"")
                .unwrap()
                .is_empty()
        );
        assert!(parser.push(b":1}\n").unwrap().is_empty());
        assert_eq!(
            parser.push(b"\n:\n\ndata: x\ndata:y\n\n").unwrap(),
            ["{\"a\":1}", "x\ny"]
        );
        assert!(parser.push(b"event: only\n\n").unwrap().is_empty());
    }

    #[test]
    fn an_oversized_event_is_discarded_even_when_complete() {
        let mut parser = EventParser::default();
        assert!(parser.push(&vec![b'a'; EVENT_LIMIT + 1]).is_err());
        assert_eq!(parser.push(b"data: next\n\n").unwrap(), ["next"]);
        let mut complete = vec![b'a'; EVENT_LIMIT];
        complete.extend_from_slice(b"\n\n");
        assert!(parser.push(&complete).is_err());
        assert_eq!(parser.push(b"data: after\n\n").unwrap(), ["after"]);
    }

    #[tokio::test]
    async fn a_mailbox_folds_undelivered_windows_and_ends_after_them() {
        let mailbox = Mailbox::default();
        mailbox.deliver(window(1, 0.9));
        mailbox.deliver(window(2, 0.1));
        mailbox.end("gone");
        mailbox.end("ignored");
        let merged = mailbox.take().await.unwrap();
        assert_eq!(merged.sequence, 2);
        assert_eq!(merged.meters.nodes[0].peaks, [0.9]);
        assert_eq!(mailbox.take().await.unwrap_err(), "gone");
        let waiting = Arc::new(Mailbox::default());
        let reader = tokio::spawn({
            let waiting = Arc::clone(&waiting);
            async move { waiting.take().await.map(|window| window.sequence) }
        });
        tokio::task::yield_now().await;
        waiting.deliver(window(3, 0.2));
        assert_eq!(reader.await.unwrap(), Ok(3));
    }

    /// Prepared windows (popped from the end), then either the end or silence.
    struct Prepared {
        windows: Vec<ControlTelemetry>,
        then_silent: bool,
    }

    impl Windows for Prepared {
        async fn next(&mut self) -> Option<ControlTelemetry> {
            tokio::task::yield_now().await;
            match self.windows.pop() {
                Some(window) => Some(window),
                None if self.then_silent => std::future::pending().await,
                None => None,
            }
        }
    }

    struct Controlled(tokio::sync::mpsc::Receiver<ControlTelemetry>);

    impl Windows for Controlled {
        async fn next(&mut self) -> Option<ControlTelemetry> {
            self.0.recv().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hidden_windows_coalesce_peaks_and_resume_immediately_with_latest_state() {
        let (windows, receiver) = tokio::sync::mpsc::channel(4);
        let (visible, mut visibility) = watch::channel(false);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let delivered = Arc::clone(&sent);
        let task = tokio::spawn(async move {
            let credit = Semaphore::new(CREDITS);
            forward(
                &mut Controlled(receiver),
                |delivery| {
                    delivered.lock().unwrap().push(delivery);
                    credit.add_permits(1);
                    true
                },
                &credit,
                &mut visibility,
            )
            .await;
        });
        windows.send(window(1, 0.2)).await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(sent.lock().unwrap().len(), 1);
        windows.send(window(2, 0.9)).await.unwrap();
        windows.send(window(3, 0.1)).await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(999)).await;
        assert_eq!(sent.lock().unwrap().len(), 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        {
            let messages = sent.lock().unwrap();
            assert_eq!(messages.len(), 2);
            let Delivery::Window(packet) = &messages[1] else {
                panic!("hidden editor must receive its current telemetry");
            };
            assert_eq!(packet.sequence, 3);
            assert_eq!(packet.meters.nodes[0].peaks, [0.9]);
            assert_eq!(packet.state.as_ref().unwrap().graph.active_revision, 3);
        }
        windows.send(window(4, 0.3)).await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(sent.lock().unwrap().len(), 2);
        visible.send(true).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(sent.lock().unwrap().len(), 3);
        {
            let messages = sent.lock().unwrap();
            let Delivery::Window(packet) = &messages[2] else {
                panic!("restored editor must receive its latest state immediately");
            };
            assert_eq!(packet.state.as_ref().unwrap().graph.active_revision, 4);
        }
        drop(windows);
        task.await.unwrap();
        assert_eq!(sent.lock().unwrap().last(), Some(&Delivery::Ended));
    }

    #[tokio::test(start_paused = true)]
    async fn losing_a_hidden_editors_visibility_owner_ends_delivery() {
        let (visible, mut visibility) = watch::channel(false);
        let mut windows = Prepared {
            windows: vec![window(1, 0.5)],
            then_silent: true,
        };
        let credit = Semaphore::new(CREDITS);
        let mut sent = Vec::new();
        drop(visible);
        forward(
            &mut windows,
            |message| {
                sent.push(message);
                true
            },
            &credit,
            &mut visibility,
        )
        .await;
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1], Delivery::Ended);
    }

    #[tokio::test(start_paused = true)]
    async fn forwarding_stops_when_the_webview_returns_no_credit() {
        for visible in [true, false] {
            let credit = Semaphore::new(1);
            let (_owner, mut visibility) = watch::channel(visible);
            let mut windows = Prepared {
                windows: vec![window(2, 0.1), window(1, 0.8)],
                then_silent: false,
            };
            let mut sent = Vec::new();
            let started = tokio::time::Instant::now();
            forward(
                &mut windows,
                |message| {
                    sent.push(message);
                    true
                },
                &credit,
                &mut visibility,
            )
            .await;
            // One credit: a single window, then the end once credit never returns.
            assert_eq!(sent.len(), 2);
            assert!(matches!(sent[0], Delivery::Window(_)));
            assert_eq!(sent[1], Delivery::Ended);
            assert_eq!(started.elapsed(), Duration::from_secs(10));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn forwarding_ends_when_the_stream_ends_or_the_channel_closes() {
        let credit = Semaphore::new(CREDITS);
        let mut sent = Vec::new();
        forward(
            &mut Prepared {
                windows: Vec::new(),
                then_silent: false,
            },
            |message| {
                sent.push(message);
                true
            },
            &credit,
            &mut watch::channel(true).1,
        )
        .await;
        assert_eq!(sent, [Delivery::Ended]);
        let mut attempts = 0;
        forward(
            &mut Prepared {
                windows: vec![window(1, 0.5)],
                then_silent: true,
            },
            |_| {
                attempts += 1;
                false
            },
            &credit,
            &mut watch::channel(true).1,
        )
        .await;
        assert_eq!(attempts, 1);
    }
}
