//! Waits for tap callbacks and accounts for how promptly the pump wakes after them.
use super::PUMP_IDLE_INTERVAL;
use super::ports::{CaptureTap, TapWake};
use mixrelay_macos_transport::application_source_transport::SourceClockSnapshot;
use mixrelay_realtime::{LatencyHistogram, LatencySummary, Timebase};
use mixrelay_time::{Deadline, Instant, MonotonicClock, Sleeper};
use std::time::Duration;

/// Wake statistics are logged at most this often.
const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// Measures how late the pump wakes after a callback it was waiting for, and counts wakes
/// it was too busy to wait for, timeouts and failures.
pub(super) struct Waker {
    timebase: Timebase,
    /// The tap's callback period; a tap wait lasts at most two of them.
    pub(super) period: Duration,
    wakes: LatencyHistogram,
    busy: u64,
    timeouts: u64,
    failures: u64,
    /// A failed notifier is not trusted again for this capture; poll instead.
    degraded: bool,
    reported: Option<Instant>,
    reported_failed_signals: u64,
}

/// One report window's wake statistics.
#[derive(Debug, PartialEq, Eq)]
struct WakeWindow {
    latency: Option<LatencySummary>,
    busy: u64,
    timeouts: u64,
    failures: u64,
    failed_signals: u64,
}

impl Waker {
    pub(super) fn new(timebase: Timebase, period: Duration) -> Self {
        Self {
            timebase,
            period,
            wakes: LatencyHistogram::default(),
            busy: 0,
            timeouts: 0,
            failures: 0,
            degraded: false,
            reported: None,
            reported_failed_signals: 0,
        }
    }

    /// Waits for the tap's next callback, at most two callback periods.
    pub(super) fn for_tap(
        &mut self,
        tap: &impl CaptureTap,
        clock: &(impl MonotonicClock + Sleeper),
    ) {
        self.wait(tap, clock, self.period * 2);
    }

    /// Short bounded wait for progress no notification reports (the engine consuming,
    /// a format settling); a tap callback may still end it early.
    pub(super) fn briefly(
        &mut self,
        tap: &impl CaptureTap,
        clock: &(impl MonotonicClock + Sleeper),
    ) {
        self.wait(tap, clock, PUMP_IDLE_INTERVAL);
    }

    /// Waits on the tap's notifier, or polls when it has none or it failed before.
    fn wait(
        &mut self,
        tap: &impl CaptureTap,
        clock: &(impl MonotonicClock + Sleeper),
        timeout: Duration,
    ) {
        let wake = if self.degraded {
            None
        } else {
            tap.wait_for_callback(timeout)
        };
        match wake {
            Some(wake) => self.record(wake),
            None => clock.sleep_until(Deadline::after(clock.now(), PUMP_IDLE_INTERVAL)),
        }
    }

    fn record(&mut self, wake: TapWake) {
        match wake {
            TapWake::Woken { late_ticks } => {
                self.wakes.record(self.timebase.nanoseconds(late_ticks));
            }
            TapWake::Missed => self.busy += 1,
            TapWake::TimedOut => self.timeouts += 1,
            TapWake::Failed => {
                self.failures += 1;
                self.degraded = true;
            }
        }
    }

    /// A replacement capture brings its own notifier.
    pub(super) fn replaced(&mut self) {
        self.degraded = false;
    }

    /// Control step: logs consumer latency and the wake window every ten seconds.
    pub(super) fn report(
        &mut self,
        tap: &impl CaptureTap,
        consumer: Option<SourceClockSnapshot>,
        now: Instant,
    ) {
        if self
            .reported
            .is_some_and(|at| now.duration_since(at) < REPORT_INTERVAL)
        {
            return;
        }
        self.reported = Some(now);
        if let Some(consumer) = consumer {
            log_consumer_latency(tap.source_id(), &consumer);
        }
        if let Some(window) = self.take_window(tap.failed_signals()) {
            self.log_window(&window);
        }
    }

    /// Takes and clears the window; `None` when nothing happened since the last one.
    fn take_window(&mut self, failed_signals: u64) -> Option<WakeWindow> {
        let window = WakeWindow {
            latency: self.wakes.take(),
            busy: std::mem::take(&mut self.busy),
            timeouts: std::mem::take(&mut self.timeouts),
            failures: std::mem::take(&mut self.failures),
            failed_signals,
        };
        let new_failed_signals =
            std::mem::replace(&mut self.reported_failed_signals, failed_signals) != failed_signals;
        let quiet = window.latency.is_none()
            && window.busy == 0
            && window.timeouts == 0
            && window.failures == 0;
        (!quiet || new_failed_signals).then_some(window)
    }

    fn log_window(&self, window: &WakeWindow) {
        let latency = window.latency.unwrap_or(LatencySummary {
            count: 0,
            p50_us: 0,
            p99_us: 0,
            p999_us: 0,
            maximum_us: 0,
        });
        tracing::info!(
            wakes = latency.count,
            p50_us = latency.p50_us,
            p99_us = latency.p99_us,
            p999_us = latency.p999_us,
            max_us = latency.maximum_us,
            busy = window.busy,
            timeouts = window.timeouts,
            failures = window.failures,
            failed_signals = window.failed_signals,
            degraded = self.degraded,
            period_us = self.period.as_micros(),
            "capture pump wakes after tap callbacks"
        );
    }
}

fn log_consumer_latency(source_id: &str, consumer: &SourceClockSnapshot) {
    tracing::info!(
        source_id = %source_id,
        epoch = consumer.epoch,
        target_frames = consumer.target_backlog_frames,
        consumer_burst_frames = consumer.consumer_burst_frames,
        starvation_episodes = consumer.starvation_episodes,
        starvation_frames = consumer.starvation_frames,
        rebuffer_frames = consumer.rebuffer_frames,
        overwrite_skips = consumer.overwrite_skips,
        insufficient_headroom = consumer.insufficient_headroom,
        "application consumer latency"
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::FakeTap;
    use super::*;
    use mixrelay_time::ManualClock;
    use std::num::NonZeroU32;

    fn waker() -> Waker {
        let unit = NonZeroU32::new(1).unwrap();
        Waker::new(Timebase::from_ratio(unit, unit), Duration::from_millis(10))
    }

    #[test]
    fn a_failed_notifier_is_polled_on_the_clock_until_the_capture_is_replaced() {
        let clock = ManualClock::default();
        let tap = FakeTap::default();
        tap.wakes
            .borrow_mut()
            .extend([TapWake::Failed, TapWake::TimedOut]);
        let mut waker = waker();
        let start = clock.now();

        waker.for_tap(&tap, &clock);
        assert_eq!(tap.waits.get(), 1);
        assert_eq!(
            clock.now(),
            start,
            "a notifier wait does not sleep on the clock"
        );

        waker.for_tap(&tap, &clock);
        waker.briefly(&tap, &clock);
        assert_eq!(
            tap.waits.get(),
            1,
            "a failed notifier is not waited on again"
        );
        assert_eq!(clock.now().duration_since(start), PUMP_IDLE_INTERVAL * 2);

        waker.replaced();
        waker.for_tap(&tap, &clock);
        assert_eq!(tap.waits.get(), 2);
        assert_eq!(
            tap.timeouts.borrow().last(),
            Some(&Duration::from_millis(20))
        );
    }

    #[test]
    fn a_tap_without_a_notifier_is_polled_at_the_idle_interval() {
        let clock = ManualClock::default();
        let tap = FakeTap::polled();
        let mut waker = waker();
        let start = clock.now();
        waker.for_tap(&tap, &clock);
        assert_eq!(clock.now().duration_since(start), PUMP_IDLE_INTERVAL);
        assert_eq!(waker.take_window(0), None);
    }

    #[test]
    fn each_report_window_counts_its_own_wakes_and_quiet_windows_are_skipped() {
        let mut waker = waker();
        for wake in [
            TapWake::Woken { late_ticks: 3_000 },
            TapWake::Missed,
            TapWake::Missed,
            TapWake::TimedOut,
        ] {
            waker.record(wake);
        }
        let window = waker.take_window(0).unwrap();
        assert_eq!(window.latency.map(|latency| latency.count), Some(1));
        assert_eq!((window.busy, window.timeouts, window.failures), (2, 1, 0));
        assert_eq!(waker.take_window(0), None, "the window was cleared");
        let window = waker.take_window(4).unwrap();
        assert_eq!(
            window.failed_signals, 4,
            "new undelivered signals are reported"
        );
        assert_eq!(waker.take_window(4), None);
    }
}
