//! Acknowledgement of one requested engine pause or resume transition.
use mixrelay_control_protocol::EnginePowerStatus;
use mixrelay_time::{Deadline, Instant};
use std::time::Duration;

/// Progress after a status observation within the original operation deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerProgress {
    Complete,
    Waiting(Deadline),
    Expired,
}

/// Tracks the requested state without allowing status polling to extend its timeout.
pub struct PowerAcknowledgement {
    enabled: bool,
    deadline: Deadline,
}
impl PowerAcknowledgement {
    /// Thirty seconds covers native drain while keeping menu actions bounded.
    #[must_use]
    pub fn new(enabled: bool, now: Instant) -> Self {
        Self {
            enabled,
            deadline: Deadline::after(now, Duration::from_secs(30)),
        }
    }
    /// The same deadline bounds every individual status read.
    #[must_use]
    pub fn deadline(&self) -> Deadline {
        self.deadline
    }
    /// A transition is complete only after the engine reports the requested stable state.
    #[must_use]
    pub fn observe(&self, status: EnginePowerStatus, now: Instant) -> PowerProgress {
        if status.enabled == self.enabled && !status.transitioning {
            return PowerProgress::Complete;
        }
        if self.deadline.has_passed(now) {
            return PowerProgress::Expired;
        }
        PowerProgress::Waiting(Deadline::at(
            self.deadline
                .instant()
                .min(now.saturating_add(Duration::from_millis(100))),
        ))
    }
}
#[cfg(test)]
mod tests;
