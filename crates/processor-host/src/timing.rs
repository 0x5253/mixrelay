//! Sampled transport duration and bounded observation generations.

use mixrelay_time::{Instant, MonotonicClock};
use std::sync::atomic::{AtomicU64, Ordering};

/// Control-thread sampling requests and the last completed transport measurement.
#[derive(Default)]
pub(crate) struct TimingMetrics {
    requested: AtomicU64,
    completed: AtomicU64,
    nanos: AtomicU64,
}

impl TimingMetrics {
    pub(crate) fn request(&self) {
        self.requested.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn latest(&self) -> Option<u64> {
        let completed = self.completed.load(Ordering::Acquire);
        let requested = self.requested.load(Ordering::Acquire);
        let nanos = self.nanos.load(Ordering::Relaxed);
        (completed != 0 && requested.saturating_sub(completed) <= 1 && nanos != 0).then_some(nanos)
    }
}

/// Audio-owner state: clock reads occur only for the one requested block each second.
#[derive(Default)]
pub(crate) struct TimingProbe {
    generation: u64,
    pending: Option<(u64, u64, Instant)>,
}

impl TimingProbe {
    pub(crate) fn submitted(
        &mut self,
        sequence: u64,
        metrics: &TimingMetrics,
        clock: &dyn MonotonicClock,
    ) {
        let generation = metrics.requested.load(Ordering::Acquire);
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        self.pending = Some((sequence, generation, clock.now()));
    }

    pub(crate) fn received(
        &mut self,
        sequence: u64,
        metrics: &TimingMetrics,
        clock: &dyn MonotonicClock,
    ) {
        let Some((expected, generation, start)) = self.pending else {
            return;
        };
        if sequence != expected {
            return;
        }
        self.pending = None;
        let nanos = u64::try_from(clock.now().duration_since(start).as_nanos()).unwrap_or(u64::MAX);
        metrics.nanos.store(nanos, Ordering::Relaxed);
        metrics.completed.store(generation, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn samples_only_requested_sequences_and_expires_stalled_measurements() {
        let clock = mixrelay_time::ManualClock::default();
        let metrics = TimingMetrics::default();
        let mut probe = TimingProbe::default();
        probe.submitted(1, &metrics, &clock);
        assert!(probe.pending.is_none());
        assert_eq!(metrics.latest(), None);
        metrics.request();
        probe.submitted(2, &metrics, &clock);
        probe.submitted(3, &metrics, &clock);
        assert_eq!(probe.pending.as_ref().unwrap().0, 2);
        probe.received(1, &metrics, &clock);
        assert_eq!(metrics.latest(), None);
        clock.advance(Duration::from_millis(1));
        probe.received(2, &metrics, &clock);
        assert_eq!(metrics.latest(), Some(1_000_000));
        let measured = metrics.latest();
        probe.received(2, &metrics, &clock);
        assert_eq!(metrics.latest(), measured);
        metrics.request();
        assert_eq!(metrics.latest(), measured);
        metrics.request();
        assert_eq!(metrics.latest(), None);
    }

    #[test]
    fn a_new_sampling_request_cannot_accept_the_old_pending_output() {
        let clock = mixrelay_time::ManualClock::default();
        let metrics = TimingMetrics::default();
        let mut probe = TimingProbe::default();
        metrics.request();
        probe.submitted(10, &metrics, &clock);
        metrics.request();
        probe.submitted(11, &metrics, &clock);
        probe.received(10, &metrics, &clock);
        assert_eq!(metrics.latest(), None);
        assert_eq!(probe.pending.as_ref().unwrap().0, 11);
        probe.received(11, &metrics, &clock);
        assert!(probe.pending.is_none());
        assert_eq!(metrics.completed.load(Ordering::Acquire), 2);
    }

    #[test]
    fn clock_reads_only_match_requested_sequences_without_callback_allocation() {
        struct Clock(AtomicU64);
        impl MonotonicClock for Clock {
            fn now(&self) -> Instant {
                Instant::from_origin(Duration::from_nanos(self.0.fetch_add(1, Ordering::Relaxed)))
            }
        }
        let clock = Clock(AtomicU64::new(0));
        let metrics = TimingMetrics::default();
        let mut probe = TimingProbe::default();

        let measured = allocation_counter::measure(|| {
            probe.submitted(1, &metrics, &clock);
            probe.received(1, &metrics, &clock);
            assert_eq!(clock.0.load(Ordering::Relaxed), 0);
            metrics.request();
            probe.submitted(8, &metrics, &clock);
            probe.submitted(9, &metrics, &clock);
            probe.received(7, &metrics, &clock);
            assert_eq!(clock.0.load(Ordering::Relaxed), 1);
            probe.received(8, &metrics, &clock);
            probe.received(8, &metrics, &clock);
            assert_eq!(clock.0.load(Ordering::Relaxed), 2);
            assert_eq!(metrics.latest(), Some(1));
        });
        assert_eq!(measured.count_total, 0);
    }
}
