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
};

use mixrelay_control_protocol::TelemetryEncoder;
use tauri::Manager as _;
use tokio::sync::{Semaphore, watch};

use forward::{Delivery, forward};

mod forward;

/// Windows the webview may hold unacknowledged.
const CREDITS: usize = 2;

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

    fn window(sequence: u64, peak: f32) -> mixrelay_control_protocol::ControlTelemetry {
        mixrelay_control_protocol::ControlTelemetry {
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
            state: None,
        }
    }
}
