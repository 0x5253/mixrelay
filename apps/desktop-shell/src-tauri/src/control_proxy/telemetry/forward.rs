//! Forwards one subscription's windows to its webview as credit allows. Undelivered windows
//! fold into one pending window, so a slow or hidden editor never accumulates messages.
use std::time::Duration;

use mixrelay_control_protocol::ControlTelemetry;
use tokio::sync::{Semaphore, watch};

use crate::control_channel::Mailbox;

/// A webview that returns no credit for this long has stopped consuming.
const CREDIT_LIMIT: Duration = Duration::from_secs(10);
// A hidden editor needs state/liveness, while its meters are not displayed. Stay
// below the two-second UI watchdog; restoring visibility bypasses this interval.
const HIDDEN_INTERVAL: Duration = Duration::from_secs(1);

/// What one webview subscriber receives, in order.
#[derive(Debug, PartialEq)]
pub(super) enum TelemetryEvent {
    Window(Box<ControlTelemetry>),
    /// The stream ended; the UI resubscribes or falls back to polling.
    Ended,
}

/// A subscription's windows, `None` once it ended.
pub(super) trait Windows: Send {
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
pub(super) async fn forward(
    windows: &mut impl Windows,
    mut send: impl FnMut(TelemetryEvent) -> bool,
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
                send(TelemetryEvent::Ended);
                return;
            };
            let visible = *visibility.borrow_and_update();
            if !visible && let Some(deadline) = next_hidden {
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {},
                    changed = visibility.changed() => {
                        if changed.is_err() {
                            send(TelemetryEvent::Ended);
                            return;
                        }
                        continue;
                    }
                }
            }
            let Ok(window) = mailbox.take().await else {
                send(TelemetryEvent::Ended);
                return;
            };
            permit.forget();
            next_hidden =
                (!*visibility.borrow()).then(|| tokio::time::Instant::now() + HIDDEN_INTERVAL);
            if !send(TelemetryEvent::Window(Box::new(window))) {
                return;
            }
        }
    };
    tokio::select! {
        () = receive => {}
        () = deliver => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use super::super::CREDITS;

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
                |event| {
                    delivered.lock().unwrap().push(event);
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
            let TelemetryEvent::Window(packet) = &messages[1] else {
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
            let TelemetryEvent::Window(packet) = &messages[2] else {
                panic!("restored editor must receive its latest state immediately");
            };
            assert_eq!(packet.state.as_ref().unwrap().graph.active_revision, 4);
        }
        drop(windows);
        task.await.unwrap();
        assert_eq!(sent.lock().unwrap().last(), Some(&TelemetryEvent::Ended));
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
        assert_eq!(sent[1], TelemetryEvent::Ended);
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
            assert!(matches!(sent[0], TelemetryEvent::Window(_)));
            assert_eq!(sent[1], TelemetryEvent::Ended);
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
        assert_eq!(sent, [TelemetryEvent::Ended]);
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
