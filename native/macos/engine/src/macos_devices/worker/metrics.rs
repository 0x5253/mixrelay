//! Atomic transfer and timing evidence; formatting belongs to the control thread.
use std::{
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::Duration,
};

#[derive(Default)]
pub(super) struct Timing {
    count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    over_ms: AtomicU64,
}

impl Timing {
    fn record(&self, elapsed: Duration) {
        let ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.count.fetch_add(1, Relaxed);
        self.total_ns.fetch_add(ns, Relaxed);
        self.max_ns.fetch_max(ns, Relaxed);
        if ns > 1_000_000 {
            self.over_ms.fetch_add(1, Relaxed);
        }
    }

    fn snapshot(&self) -> TimingSnapshot {
        let calls = self.count.load(Relaxed);
        TimingSnapshot {
            calls,
            mean_ns: self.total_ns.load(Relaxed) / calls.max(1),
            max_ns: self.max_ns.load(Relaxed),
            over_ms: self.over_ms.load(Relaxed),
        }
    }
}

/// Cumulative callback work timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::macos_devices) struct TimingSnapshot {
    pub(in crate::macos_devices) calls: u64,
    pub(in crate::macos_devices) mean_ns: u64,
    pub(in crate::macos_devices) max_ns: u64,
    pub(in crate::macos_devices) over_ms: u64,
}

#[derive(Default)]
pub(in crate::macos_devices) struct PcmMetrics {
    work: Timing,
    resets: [AtomicU64; 5],
    over_budget: AtomicU64,
    correction_bits: AtomicU64,
    pending_us: AtomicU64,
    peak_pending_us: AtomicU64,
    source_frames: AtomicU64,
    destination_frames: AtomicU64,
}

#[derive(Clone, Copy)]
pub(in crate::macos_devices) enum ResetCause {
    Overflow,
    ClockGap,
    DirectExit,
    Activity,
    Format,
}

impl PcmMetrics {
    pub(in crate::macos_devices) fn work_timing(&self) -> TimingSnapshot {
        self.work.snapshot()
    }

    pub(in crate::macos_devices) fn work(&self, elapsed: Duration, frames: u32, rate: u32) {
        self.work.record(elapsed);
        if frames != 0 && elapsed.as_nanos() * u128::from(rate) > u128::from(frames) * 1_000_000_000
        {
            self.over_budget.fetch_add(1, Relaxed);
        }
    }

    pub(in crate::macos_devices) fn reset(&self, cause: ResetCause) {
        self.resets[cause as usize].fetch_add(1, Relaxed);
    }

    pub(in crate::macos_devices) fn buffering(&self, frames: usize, rate: u32, correction: f64) {
        let micros = u64::try_from(frames)
            .unwrap_or(u64::MAX)
            .saturating_mul(1_000_000)
            / u64::from(rate.max(1));
        self.pending_us.store(micros, Relaxed);
        self.peak_pending_us.fetch_max(micros, Relaxed);
        self.correction_bits.store(correction.to_bits(), Relaxed);
    }

    pub(in crate::macos_devices) fn transferred(&self, source: u32, destination: u32) {
        self.source_frames.fetch_add(u64::from(source), Relaxed);
        self.destination_frames
            .fetch_add(u64::from(destination), Relaxed);
    }

    /// Cumulative approximate snapshots, never a transactional cross-thread state.
    pub(in crate::macos_devices) fn report(&self, owner: u64) {
        let TimingSnapshot {
            calls,
            mean_ns: mean_work_ns,
            max_ns: max_work_ns,
            over_ms: work_over_ms,
        } = self.work_timing();
        tracing::info!(target: "audio_trace", owner, calls, mean_work_ns,
            max_work_ns, work_over_ms, over_budget = self.over_budget.load(Relaxed),
            reset_overflow = self.resets[ResetCause::Overflow as usize].load(Relaxed),
            reset_clock = self.resets[ResetCause::ClockGap as usize].load(Relaxed),
            reset_direct_exit = self.resets[ResetCause::DirectExit as usize].load(Relaxed),
            reset_activity = self.resets[ResetCause::Activity as usize].load(Relaxed),
            reset_format = self.resets[ResetCause::Format as usize].load(Relaxed),
            correction_ppm = f64::from_bits(self.correction_bits.load(Relaxed)),
            pending_us = self.pending_us.load(Relaxed), peak_pending_us = self.peak_pending_us.load(Relaxed),
            source_frames = self.source_frames.load(Relaxed),
            destination_frames = self.destination_frames.load(Relaxed),
            "PCM callback execution timing");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_recovery_and_transfer_counts_are_independent() {
        let metrics = PcmMetrics::default();
        metrics.work(Duration::from_micros(10), 32, 48_000);
        metrics.work(Duration::from_micros(30), 32, 48_000);
        metrics.reset(ResetCause::Overflow);
        metrics.transferred(128, 64);
        assert_eq!(
            metrics.work_timing(),
            TimingSnapshot {
                calls: 2,
                mean_ns: 20_000,
                max_ns: 30_000,
                over_ms: 0,
            }
        );
        assert_eq!(metrics.resets[0].load(Relaxed), 1);
        metrics.buffering(96, 96_000, -1.25);
        assert_eq!(metrics.pending_us.load(Relaxed), 1000);
        assert_eq!(metrics.correction_bits.load(Relaxed), (-1.25_f64).to_bits());
        assert_eq!(metrics.source_frames.load(Relaxed), 128);
        assert_eq!(metrics.destination_frames.load(Relaxed), 64);
    }
}
