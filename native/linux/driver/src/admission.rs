//! Exact native tuple admission with explicit monotonic time and observations.

use mixrelay_audio_io::NativeStreamFormat;
use mixrelay_time::{Deadline, MonotonicClock, Sleeper};
use std::time::Duration;

// Native negotiation previously used this three-second control budget and 2 ms polling.
const ADMISSION_BUDGET: Duration = Duration::from_secs(3);
const OBSERVATION_STEP: Duration = Duration::from_millis(2);

pub(super) trait ControlTime: MonotonicClock + Sleeper {}
impl<T: MonotonicClock + Sleeper> ControlTime for T {}

#[derive(Debug, Clone, Copy)]
pub(super) enum DirectionEvidence {
    Absent,
    Unobserved,
    Current(Option<NativeStreamFormat>),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Evidence {
    pub(super) directions: [DirectionEvidence; 2],
    pub(super) ready: bool,
    pub(super) processing_failed: bool,
    pub(super) disconnected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum AdmissionError {
    #[error("native device processing failed before admission")]
    ProcessingFailed,
    #[error("native device disconnected before admission")]
    Disconnected,
    #[error("native device tuple readback timed out")]
    Timeout,
}

pub(super) fn wait(
    requested: [Option<NativeStreamFormat>; 2],
    clock: &dyn ControlTime,
    mut observe: impl FnMut() -> Evidence,
) -> Result<(), AdmissionError> {
    let deadline = Deadline::after(clock.now(), ADMISSION_BUDGET);
    loop {
        let evidence = observe();
        let exact = evidence
            .directions
            .iter()
            .zip(requested)
            .all(|(observed, requested)| match observed {
                DirectionEvidence::Absent => requested.is_none(),
                DirectionEvidence::Unobserved => false,
                DirectionEvidence::Current(current) => *current == requested,
            });
        if exact && evidence.ready {
            return Ok(());
        }
        if evidence.processing_failed {
            return Err(AdmissionError::ProcessingFailed);
        }
        if evidence.disconnected {
            return Err(AdmissionError::Disconnected);
        }
        let now = clock.now();
        if deadline.has_passed(now) {
            return Err(AdmissionError::Timeout);
        }
        clock.sleep_until(Deadline::at(
            now.saturating_add(OBSERVATION_STEP).min(deadline.instant()),
        ));
    }
}

#[cfg(test)]
mod tests;
