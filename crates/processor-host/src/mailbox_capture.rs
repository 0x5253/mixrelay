//! Blocking native-state capture over the isolated worker's shared mailbox.

use std::{
    sync::{Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{
    HostedProcessorControl, ProcessorStateError, editor_surface, shared_transport::ControlMailbox,
    state_capture::StateCapture,
};

impl HostedProcessorControl {
    /// Composes the native mailbox adapter with this prepared control owner.
    pub(super) fn native(
        shared: ControlMailbox,
        clock: Arc<dyn mixrelay_time::MonotonicClock>,
    ) -> Self {
        let shared = Arc::new(shared);
        let capture = Box::new(MailboxCapture {
            shared: Arc::clone(&shared),
        });
        Self::new(shared, clock, capture)
    }
}

struct MailboxCapture {
    shared: Arc<ControlMailbox>,
}

impl StateCapture for MailboxCapture {
    fn snapshot(&self, timeout: Duration) -> Result<Vec<u8>, ProcessorStateError> {
        let header = self.shared.header();
        header
            .editor_input
            .cancel(header.editor_sequence.load(Ordering::Acquire));
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = self
                .shared
                .try_lock()
                .and_then(|mut guard| guard.request_blob(editor_surface::NATIVE_STATE, remaining));
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(ProcessorStateError::Busy | ProcessorStateError::Pending)
                    if !remaining.is_zero()
                        && self.shared.last_request_kind() == editor_surface::EDITOR_FRAME =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests;
