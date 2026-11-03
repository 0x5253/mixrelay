//! Monotonic control-thread waits; audio callbacks never access this port.
use mixrelay_time::{Deadline, MonotonicClock, Sleeper};
use std::time::Duration;

pub(crate) trait ControlTime: MonotonicClock + Sleeper {}
impl<T: MonotonicClock + Sleeper> ControlTime for T {}

// Preserve the native manager negotiation allowance and the existing polling cadence.
pub(crate) const PREPARATION_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(2);

pub(crate) fn poll(time: &dyn ControlTime, deadline: Deadline) {
    let next = Deadline::after(time.now(), POLL_INTERVAL).min(deadline);
    time.sleep_until(next);
}

pub(crate) enum Observation<T> {
    Pending,
    Ready(T),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WaitError {
    #[error("native preparation failed")]
    Failed,
    #[error("native preparation timed out")]
    TimedOut,
}

pub(crate) fn wait_for<T>(
    time: &dyn ControlTime,
    mut observe: impl FnMut() -> Observation<T>,
) -> Result<T, WaitError> {
    let deadline = Deadline::after(time.now(), PREPARATION_TIMEOUT);
    loop {
        match observe() {
            Observation::Ready(value) => return Ok(value),
            Observation::Failed => return Err(WaitError::Failed),
            Observation::Pending => {}
        }
        if deadline.has_passed(time.now()) {
            return Err(WaitError::TimedOut);
        }
        poll(time, deadline);
    }
}

#[cfg(test)]
mod tests;
