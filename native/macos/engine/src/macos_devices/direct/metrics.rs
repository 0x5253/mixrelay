//! Callback execution evidence, separate from queue-transfer XRUN counters.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[derive(Default)]
pub(super) struct CallbackMetrics {
    calls: AtomicU64,
    frames: AtomicU64,
    missed: AtomicU64,
    over_budget: AtomicU64,
    maximum_load: AtomicU64,
}

impl CallbackMetrics {
    #[cfg(test)]
    pub(super) fn missed_count(&self) -> u64 {
        self.missed.load(Ordering::Relaxed)
    }

    pub(super) fn missed(&self) {
        self.missed.fetch_add(1, Ordering::Relaxed);
    }

    /// A duration over one quantum is a scheduling-risk observation, not proof
    /// that `CoreAudio` discarded output. This measures the Rust renderer only,
    /// excluding native buffer validation and metric publication overhead.
    /// No logging or allocation occurs here.
    pub(super) fn record(&self, elapsed: Duration, frames: u32, rate: u32) {
        let capacity = u128::from(frames) * 1_000_000_000;
        if capacity == 0 || rate == 0 {
            return;
        }
        let used = elapsed.as_nanos().saturating_mul(u128::from(rate));
        let load = used.saturating_mul(1000) / capacity;
        self.maximum_load
            .fetch_max(u64::try_from(load).unwrap_or(u64::MAX), Ordering::Relaxed);
        if used > capacity {
            self.over_budget.fetch_add(1, Ordering::Relaxed);
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(u64::from(frames), Ordering::Relaxed);
    }

    /// Control-side diagnostic window. Independent counters may straddle a
    /// concurrent callback; they are not a transactional audio-state snapshot.
    pub(super) fn report(&self, clock_id: u32, elapsed: Duration) {
        let calls = self.calls.swap(0, Ordering::Relaxed);
        let frames = self.frames.swap(0, Ordering::Relaxed);
        let missed = self.missed.swap(0, Ordering::Relaxed);
        let over_budget = self.over_budget.swap(0, Ordering::Relaxed);
        let maximum_load_permille = self.maximum_load.swap(0, Ordering::Relaxed);
        tracing::info!(
            clock_id,
            calls,
            frames,
            window_ms = elapsed.as_millis(),
            missed,
            over_budget,
            maximum_load_permille,
            "CoreAudio Rust renderer execution"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_uses_each_native_quantum_and_does_not_call_slowness_a_drop() {
        let metrics = CallbackMetrics::default();
        metrics.record(Duration::from_micros(500), 32, 48_000);
        metrics.record(Duration::from_micros(750), 32, 48_000);
        metrics.record(Duration::from_micros(750), 64, 48_000);
        assert_eq!(metrics.calls.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.frames.load(Ordering::Relaxed), 128);
        assert_eq!(metrics.over_budget.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.maximum_load.load(Ordering::Relaxed), 1125);
        assert_eq!(metrics.missed.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn invalid_geometry_does_not_divide_by_zero_and_actual_skips_are_separate() {
        let metrics = CallbackMetrics::default();
        metrics.record(Duration::from_millis(1), 0, 48_000);
        metrics.record(Duration::from_millis(1), 32, 0);
        metrics.missed();
        assert_eq!(metrics.calls.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.missed.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.over_budget.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn completed_windows_do_not_repeat_previous_counters() {
        let metrics = CallbackMetrics::default();
        metrics.record(Duration::from_millis(1), 32, 48_000);
        metrics.missed();
        metrics.report(7, Duration::from_secs(5));
        assert_eq!(metrics.calls.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.frames.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.over_budget.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.maximum_load.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.missed_count(), 0);
        metrics.record(Duration::from_micros(250), 32, 48_000);
        assert_eq!(metrics.calls.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.frames.load(Ordering::Relaxed), 32);
        assert_eq!(metrics.over_budget.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.maximum_load.load(Ordering::Relaxed), 375);
    }
}
