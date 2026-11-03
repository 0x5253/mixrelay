//! Driver activation accepts only the committed version and its current launchd owner.

use mixrelay_driver_control::DriverState;
use mixrelay_time::{Deadline, MonotonicClock, Sleeper};
use std::time::Duration;

// Driver startup includes HAL registration and XPC inventory publication.
const READY_BUDGET: Duration = Duration::from_secs(30);
const READY_POLL: Duration = Duration::from_millis(250);

pub(crate) struct DriverObservation {
    pub(crate) state: DriverState,
    pub(crate) installed_version: Option<String>,
    pub(crate) owner_process_id: Option<u32>,
    pub(crate) registered_pid: Option<u32>,
}

/// Reads the response before launchd so a replaced service cannot satisfy readiness.
pub(crate) trait DriverReadiness {
    type Error;
    fn observe(&mut self) -> Result<DriverObservation, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadinessError<E> {
    #[error("Driver observation failed: {0}")]
    Observation(E),
    #[error("Driver is not Ready ({0:?})")]
    NotReady(DriverState),
    #[error("Driver release version differs from the committed HAL")]
    VersionMismatch,
    #[error("registered Driver has no live PID")]
    MissingRegisteredProcess,
    #[error("Driver snapshot owner differs from the current launchd PID")]
    OwnerMismatch,
}

fn validate<E>(observed: &DriverObservation, version: &str) -> Result<(), ReadinessError<E>> {
    if observed.state != DriverState::Ready {
        return Err(ReadinessError::NotReady(observed.state));
    }
    if observed.installed_version.as_deref() != Some(version) {
        return Err(ReadinessError::VersionMismatch);
    }
    let Some(pid) = observed.registered_pid else {
        return Err(ReadinessError::MissingRegisteredProcess);
    };
    if pid == 0 || observed.owner_process_id != Some(pid) {
        return Err(ReadinessError::OwnerMismatch);
    }
    Ok(())
}

pub(crate) fn wait_until_ready<P: DriverReadiness>(
    version: &str,
    probe: &mut P,
    clock: &(impl MonotonicClock + Sleeper),
) -> Result<(), ReadinessError<P::Error>> {
    let deadline = Deadline::after(clock.now(), READY_BUDGET);
    loop {
        let result = probe
            .observe()
            .map_err(ReadinessError::Observation)
            .and_then(|observed| validate(&observed, version));
        match result {
            Ok(()) => return Ok(()),
            Err(error) if deadline.has_passed(clock.now()) => return Err(error),
            Err(_) => clock.sleep_until(Deadline::at(
                clock
                    .now()
                    .saturating_add(READY_POLL)
                    .min(deadline.instant()),
            )),
        }
    }
}

#[cfg(test)]
mod tests;
