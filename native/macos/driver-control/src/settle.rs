//! Bounded HAL configuration steps and the rollback that follows a failed transaction.
//!
//! While the HAL performs a step it processes no newer inventory, so a rollback
//! published earlier would spend its own budget queued behind that step. A step
//! therefore returns only once the HAL has applied or rejected it, or the settle
//! bound ends. A step applied late still succeeds: rolling it back would cost a
//! second Perform and undo a configuration the HAL holds.
use mixrelay_driver_control::DriverError;
use mixrelay_time::{Deadline, MonotonicClock, Sleeper};
use std::time::Duration;

/// HAL budget for applying one configuration step in normal operation.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(6);
/// Further time a step that missed `ACK_TIMEOUT` may take to settle before the
/// transaction reports it pending. It covers the slowest observed Perform (9.4 s on
/// Intel runners) twice over; raising it delays only the pending report.
pub(crate) const SETTLE_TIMEOUT: Duration = Duration::from_secs(24);
// Control-only polling interval, never audio pacing or a callback wait.
const POLL: Duration = Duration::from_millis(10);

/// What the Driver service observes about one HAL step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Applied,
    /// The HAL aborted or failed the step and is idle again.
    Rejected,
    Performing,
    /// The HAL reports with another protocol version than this service.
    Incompatible,
}

impl Progress {
    pub(crate) fn applied(applied: bool) -> Self {
        if applied {
            Self::Applied
        } else {
            Self::Performing
        }
    }
}

/// Clock and pause between observations.
pub(crate) trait Pacing: MonotonicClock + Sleeper {
    fn pause(&self) {
        self.sleep_until(Deadline::after(self.now(), POLL));
    }
}

impl<C: MonotonicClock + Sleeper> Pacing for C {}

/// Why a step did not apply.
#[derive(Debug)]
pub(crate) enum Failure {
    /// Rejected before or during the step; the HAL is not performing it.
    Rejected(DriverError),
    /// Still performing when the settle bound ended.
    Pending,
    /// The HAL speaks another protocol version; waiting cannot succeed.
    Incompatible,
}

/// An error raised before the HAL was asked to perform anything.
impl From<DriverError> for Failure {
    fn from(error: DriverError) -> Self {
        Self::Rejected(error)
    }
}

impl From<Failure> for DriverError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Rejected(error) => error,
            Failure::Pending => Self::ConfigurationPending,
            Failure::Incompatible => Self::Unavailable(
                "the MixRelay HAL and Driver service use different protocol versions; \
                 reinstall MixRelay"
                    .into(),
            ),
        }
    }
}

/// Waits for one step to apply or be rejected, through `ACK_TIMEOUT` and then the
/// settle bound. A step applied within the bound succeeds; a late one is logged.
pub(crate) fn step(
    pacing: &(impl Pacing + ?Sized),
    mut observe: impl FnMut() -> Progress,
) -> Result<(), Failure> {
    let start = pacing.now();
    let mut deadline = start.saturating_add(ACK_TIMEOUT);
    let mut late = false;
    loop {
        match observe() {
            Progress::Applied => {
                if late {
                    let elapsed = pacing.now().duration_since(start);
                    tracing::warn!(
                        elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        budget_ms = u64::try_from(ACK_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                        "HAL applied a configuration step after its acknowledgement budget"
                    );
                }
                return Ok(());
            }
            Progress::Rejected => {
                return Err(Failure::Rejected(DriverError::Backend(
                    "HAL rejected the configuration".into(),
                )));
            }
            Progress::Incompatible => return Err(Failure::Incompatible),
            Progress::Performing => {}
        }
        if pacing.now() >= deadline {
            if late {
                return Err(Failure::Pending);
            }
            late = true;
            deadline = deadline.saturating_add(SETTLE_TIMEOUT);
        }
        pacing.pause();
    }
}

/// Runs `forward`; after a settled failure, publishes the previous state and waits for
/// it with a fresh budget. A step still pending, or a HAL on another protocol version,
/// gets the rollback published without waiting, so the HAL converges to the state the
/// service keeps once it can.
pub(crate) fn transact<T>(
    forward: impl FnOnce() -> Result<(), Failure>,
    publish_rollback: impl FnOnce() -> Result<T, DriverError>,
    restore: impl FnOnce(T) -> Result<(), Failure>,
) -> Result<(), DriverError> {
    let Err(failure) = forward() else {
        return Ok(());
    };
    let rollback = publish_rollback()?;
    if matches!(failure, Failure::Pending | Failure::Incompatible) {
        return Err(failure.into());
    }
    match restore(rollback) {
        Ok(()) => Err(failure.into()),
        Err(unsettled @ (Failure::Pending | Failure::Incompatible)) => Err(unsettled.into()),
        Err(Failure::Rejected(rollback)) => Err(DriverError::Backend(format!(
            "{}; rollback failed: {rollback}",
            DriverError::from(failure)
        ))),
    }
}

#[cfg(test)]
mod tests;
