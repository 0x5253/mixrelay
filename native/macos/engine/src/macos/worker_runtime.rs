//! Realtime scheduling, event-driven waits and per-path statistics for the graph worker.
//!
//! The worker renders graph blocks in the time-constraint class and steps out of it for
//! control work (activation, checkpoints, format gating, direct-mode hand-off). While a
//! device clock paces it, the worker sleeps until that device's callback publishes credit.
//! A reporting thread logs paths and wake latency every ten seconds and drains the
//! last partial window during shutdown.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

mod reporting;
use super::external_io::TimeReadMisses;
use reporting::Reporter;

use mixrelay_realtime::{
    LatencyHistogram, LatencySummary, Notifier, RealtimeThread, Timebase, Transition, Wake,
};

/// Clock IDs `next_native_pace` reports for the pacing master.
pub(super) const HARDWARE_CLOCK: u64 = 2;
pub(super) const SHARED_SOURCE_CLOCK: u64 = 3;

/// Polling interval when no device callback can wake the worker (app capture or timer
/// pacing, or no wakeup semaphore).
const POLL: Duration = Duration::from_micros(250);
/// Longest wait for device credit before the worker re-examines its clocks anyway.
const LONGEST_WAIT: Duration = Duration::from_millis(5);
const REPORT_EVERY: Duration = Duration::from_secs(10);

/// Execution paths, counted per report window.
#[derive(Clone, Copy)]
pub(super) enum Path {
    /// The graph moved into a device `IOProc` (direct rendering).
    Direct,
    Hardware,
    SharedSource,
    Timer,
}

#[derive(Default, Clone, Copy)]
struct Counts {
    direct: u64,
    hardware: u64,
    shared_source: u64,
    timer: u64,
    idle_waits: u64,
    timeouts: u64,
    time_reads: TimeReadMisses,
}

/// One report window, handed to the reporting thread.
struct Window {
    counts: Counts,
    latency: Option<LatencySummary>,
    realtime: bool,
}

pub(super) struct WorkerRuntime {
    realtime: Option<RealtimeThread>,
    attempted: bool,
    period: Duration,
    wake: Option<Arc<Notifier>>,
    timebase: Timebase,
    latency: LatencyHistogram,
    counts: Counts,
    reported: Instant,
    reporter: Option<Reporter>,
}

/// Time-constraint budget for a block period: compute within 40 %, finish within 80 %.
fn budget(period: Duration) -> (Duration, Duration, Duration) {
    (period, period.mul_f64(0.4), period.mul_f64(0.8))
}

impl WorkerRuntime {
    pub(super) fn new(wake: Option<Arc<Notifier>>) -> Self {
        Self {
            realtime: None,
            attempted: false,
            period: Duration::ZERO,
            wake,
            timebase: Timebase::current(),
            latency: LatencyHistogram::default(),
            counts: Counts::default(),
            reported: Instant::now(),
            reporter: Reporter::start(reporting::log)
                .inspect_err(|error| tracing::warn!(%error, "graph worker paths are not reported"))
                .ok(),
        }
    }

    /// Runs control work at standard priority (property queries, allocation, joins, waits
    /// that are not audio deadlines), then returns to the realtime class.
    pub(super) fn off_realtime<R>(&self, work: impl FnOnce() -> R) -> R {
        let Some(realtime) = &self.realtime else {
            return work();
        };
        let (result, transition) = realtime.outside(work);
        match transition {
            Ok(()) => {}
            Err(Transition::Demote(error)) => {
                tracing::warn!(%error, "graph worker ran control work in the realtime class");
            }
            Err(Transition::Promote(error)) => {
                tracing::warn!(%error, "graph worker could not re-enter the realtime class");
            }
        }
        result
    }

    /// Enters (once) or retunes the realtime class for blocks of `period`.
    pub(super) fn render_at(&mut self, period: Duration) {
        if period.is_zero() || period == self.period {
            return;
        }
        self.period = period;
        let (period, computation, constraint) = budget(period);
        if let Some(realtime) = &self.realtime {
            // A new budget applies on re-entry; re-enter now so this block already has it.
            realtime.retune(period, computation, constraint);
            self.off_realtime(|| ());
            return;
        }
        if std::mem::replace(&mut self.attempted, true) {
            return;
        }
        self.realtime = RealtimeThread::promote(period, computation, constraint)
            .inspect_err(|error| tracing::warn!(%error, "graph worker runs at standard priority"))
            .ok();
    }

    /// Leaves the realtime class for good (worker shutdown) and stops the reporter.
    pub(super) fn finish(&mut self) {
        let window = self.take_window();
        if self.realtime.take().is_some()
            && let Err(error) = mixrelay_realtime::demote_current_thread()
        {
            tracing::warn!(%error, "graph worker stops in the realtime class");
        }
        if let Some(reporter) = self.reporter.take() {
            reporter.finish(window);
        }
    }

    pub(super) fn record(&mut self, path: Path) {
        let count = match path {
            Path::Direct => &mut self.counts.direct,
            Path::Hardware => &mut self.counts.hardware,
            Path::SharedSource => &mut self.counts.shared_source,
            Path::Timer => &mut self.counts.timer,
        };
        *count += 1;
        // Entering direct mode may last indefinitely: close the window now.
        self.report(matches!(path, Path::Direct));
    }

    /// Adds one graph cycle's time-addressed read misses to the current window.
    pub(super) fn record_time_reads(&mut self, misses: TimeReadMisses) {
        self.counts.time_reads += misses;
    }

    /// Waits for frame credit. A device clock wakes the worker from its callback; other
    /// masters (shared source producers, timer) are polled.
    pub(super) fn wait_for_credit(&mut self, clock_id: u64) {
        self.counts.idle_waits += 1;
        let Some(wake) = self.wake.as_ref().filter(|_| clock_id == HARDWARE_CLOCK) else {
            std::thread::sleep(POLL);
            self.report(false);
            return;
        };
        let longest = self.period.saturating_mul(2).clamp(POLL, LONGEST_WAIT);
        let waiting_since = mixrelay_realtime::host_time();
        match wake.wait(longest) {
            // Only a publication made while this thread waited measures wake latency; an
            // older pending token (left from direct mode or a previous block) does not.
            Wake::Notified { published_at } if published_at >= waiting_since => {
                let now = mixrelay_realtime::host_time();
                self.latency
                    .record(self.timebase.nanoseconds(now.saturating_sub(published_at)));
            }
            Wake::Notified { .. } => {}
            Wake::TimedOut => self.counts.timeouts += 1,
            Wake::Failed(_) => std::thread::sleep(POLL),
        }
        self.report(false);
    }

    /// Hands the window to the reporting thread every ten seconds (or now); never blocks.
    fn report(&mut self, now: bool) {
        if !now && self.reported.elapsed() < REPORT_EVERY {
            return;
        }
        self.reported = Instant::now();
        let window = self.take_window();
        // A busy slot (the reporter is reading it) drops this window rather than wait.
        if let Some(reporter) = &self.reporter {
            reporter.publish(window);
        }
    }

    fn take_window(&mut self) -> Window {
        Window {
            counts: std::mem::take(&mut self.counts),
            latency: self.latency.take(),
            realtime: self.realtime.is_some(),
        }
    }
}

#[cfg(test)]
mod tests;

impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        self.finish();
    }
}
