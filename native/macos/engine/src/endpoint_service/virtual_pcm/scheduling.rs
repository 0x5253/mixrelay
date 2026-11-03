//! Scheduling policy belongs to the virtual source worker, outside native callbacks.
use super::Error;
use mixrelay_macos_transport::client_pcm::ClientPcmClock;
use mixrelay_realtime::{RealtimeThread, Timebase, demote_current_thread, host_time, wait_until};
use std::{cell::Cell, time::Duration};

/// Format/grant handshakes have no admitted native processing deadline yet.
const CONTROL_POLL: Duration = Duration::from_micros(250);
/// Bound stop and membership observation even for invalid or future clock metadata.
const MAX_CLOCK_WAIT: Duration = Duration::from_millis(5);

pub(super) fn wait_for_period(clock: Option<ClientPcmClock>, timebase: Timebase) {
    let Some(clock) = clock.filter(|clock| clock.running) else {
        std::thread::sleep(CONTROL_POLL);
        return;
    };
    let now = host_time();
    let elapsed = timebase.nanoseconds(now.saturating_sub(clock.anchor));
    let Some(next) = super::cadence::next(elapsed, clock.rate) else {
        std::thread::sleep(CONTROL_POLL);
        return;
    };
    let deadline = clock.anchor.saturating_add(timebase.ticks(next));
    let limit = now.saturating_add(timebase.ticks(MAX_CLOCK_WAIT));
    wait_until(deadline.min(limit));
}

pub(super) struct Scheduling {
    realtime: Option<RealtimeThread>,
    failed_transitions: Cell<u64>,
}
impl Scheduling {
    pub(super) fn new() -> Self {
        let period = Duration::from_secs_f64(f64::from(super::cadence::FRAMES) / 48_000.0);
        let realtime = RealtimeThread::promote(period, period * 2 / 5, period * 4 / 5)
            .inspect_err(|error| tracing::warn!(boundary = "virtual_pcm_schedule", %error, "virtual input uses standard scheduling"))
            .ok();
        Self {
            realtime,
            failed_transitions: Cell::new(0),
        }
    }
    /// Keeps standard scheduling, for tests that drive cycles on their own thread.
    #[cfg(test)]
    pub(super) fn standard() -> Self {
        Self {
            realtime: None,
            failed_transitions: Cell::new(0),
        }
    }
    pub(super) fn retune(&self, rate: u32) {
        if let Some(realtime) = &self.realtime {
            let period =
                Duration::from_secs_f64(f64::from(super::cadence::FRAMES) / f64::from(rate));
            realtime.retune(period, period * 2 / 5, period * 4 / 5);
        }
    }
    pub(super) fn outside<R>(&self, operation: impl FnOnce() -> R) -> R {
        let Some(realtime) = &self.realtime else {
            return operation();
        };
        let (result, transition) = realtime.outside(operation);
        if transition.is_err() {
            self.failed_transitions
                .set(self.failed_transitions.get().saturating_add(1));
        }
        result
    }
    pub(super) fn finish(self) -> Result<(), Error> {
        demote_current_thread().map_err(Error::Scheduling)?;
        if self.failed_transitions.get() != 0 {
            tracing::warn!(
                boundary = "virtual_pcm_schedule",
                failed_transitions = self.failed_transitions.get(),
                "virtual input scheduling transitions failed"
            );
        }
        Ok(())
    }
}
