//! Shared editor/runtime channel bounds and one pending telemetry window per reader.
use mixrelay_control_protocol::ControlTelemetry;
use std::sync::{Mutex, PoisonError};
use tokio::sync::Notify;

pub(crate) use mixrelay_control_protocol::runtime_link::{
    FRAME_LIMIT, LINK_PROTOCOL, RESPONSE_LIMIT, SILENCE_LIMIT,
};

/// One reader's undelivered window. Newer windows fold into it, so a slow reader costs a
/// single window of memory and still sees every peak since its last delivery.
#[derive(Default)]
pub(crate) struct Mailbox {
    state: Mutex<Pending>,
    ready: Notify,
}

#[derive(Default)]
struct Pending {
    window: Option<ControlTelemetry>,
    ended: Option<String>,
}

impl Mailbox {
    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn deliver(&self, window: ControlTelemetry) {
        let mut pending = self.pending();
        match &mut pending.window {
            Some(older) => older.absorb(window),
            slot @ None => *slot = Some(window),
        }
        drop(pending);
        self.ready.notify_one();
    }

    /// Ends the stream after any pending window; the first reason wins.
    pub(crate) fn end(&self, reason: &str) {
        self.pending()
            .ended
            .get_or_insert_with(|| reason.to_owned());
        self.ready.notify_one();
    }

    /// The pending window, or once none remains, why the stream ended.
    pub(crate) async fn take(&self) -> Result<ControlTelemetry, String> {
        loop {
            {
                let mut pending = self.pending();
                if let Some(window) = pending.window.take() {
                    return Ok(window);
                }
                if let Some(reason) = &pending.ended {
                    return Err(reason.clone());
                }
            }
            // `notify_one` stores a permit, so a delivery between the check and here wakes us.
            self.ready.notified().await;
        }
    }
}

/// The linked desktop target owns the control transport's OS identity.
pub(crate) const fn failure_platform() -> mixrelay_control_protocol::ControlPlatform {
    #[cfg(target_os = "macos")]
    {
        mixrelay_control_protocol::ControlPlatform::MacOs
    }
    #[cfg(target_os = "linux")]
    {
        mixrelay_control_protocol::ControlPlatform::Linux
    }
}
