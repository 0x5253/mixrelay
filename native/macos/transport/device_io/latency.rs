//! Callback host intervals and host-timed FIFO passages, never nominal targets.
use super::{DeviceIo, IoTiming};
use mixrelay_audio_io::residence::{Passage, PassageSnapshot, Residence, ResidenceSnapshot};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct Measurements {
    pub(super) faults: AtomicU64,
    pub(super) acquisition: Residence,
    pub(super) presentation: Residence,
    pub(super) direct_acquisition: Residence,
    pub(super) direct_presentation: Residence,
    pub(super) input_scalar: Residence,
    pub(super) output_scalar: Residence,
    pub(super) direct_input_scalar: Residence,
    pub(super) direct_output_scalar: Residence,
    pub(super) native_queue: Passage,
    pub(super) graph_queue: Passage,
    /// Graph consumer's host label minus the exact acquired PCM coordinates, in ns.
    pub(super) content_age: Residence,
}

impl Measurements {
    /// Records one callback's timing for the `frames` it moved. Every tap is weighted by
    /// frames, like FIFO residence, so a window mixing quanta keeps a per-frame mean.
    pub(super) fn callback(&self, timing: IoTiming, frames: u32, direct: bool) {
        let weight = u64::from(frames);
        let (input, output) = if direct {
            (&self.direct_acquisition, &self.direct_presentation)
        } else {
            (&self.acquisition, &self.presentation)
        };
        input.record(interval(timing.now, timing.input), weight);
        output.record(interval(timing.output, timing.now), weight);
        let (input, output) = if direct {
            (&self.direct_input_scalar, &self.direct_output_scalar)
        } else {
            (&self.input_scalar, &self.output_scalar)
        };
        scalar(input, timing.input, weight);
        scalar(output, timing.output, weight);
    }
}

// Fixed-point scalars preserve a coherent sum/count using the existing atomic tap.
// Absent or invalid scalars are skipped, so a window with none falls back to nominal.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated finite positive fixed-point scalar"
)]
fn scalar(tap: &Residence, timestamp: super::IoTimestamp, weight: u64) {
    let value = timestamp.rate_scalar * 1e9;
    if timestamp.flags & 4 != 0 && value.is_finite() && (1.0..1e12).contains(&value) {
        tap.record(Some(value.round() as u64), weight);
    }
}

fn interval(later: super::IoTimestamp, earlier: super::IoTimestamp) -> Option<u64> {
    (later.flags & earlier.flags & 2 != 0)
        .then(|| later.host_time.checked_sub(earlier.host_time))
        .flatten()
}

/// Called while the graph consumer's reservation retains every PCM/metadata slot.
/// Unknown/future coordinates invalidate the observation, without borrowing another
/// callback's time. The mean is computed once per block and weighted by its frames.
pub(super) fn capture_age(io: &DeviceIo, cursor: u64, frames: u32, host: u64) -> Option<u64> {
    let label = io.timebase.nanoseconds(host);
    let sum = (0..frames).try_fold(0_u128, |sum, offset| {
        let acquired = io.graph_times.at(cursor + u64::from(offset))?;
        (acquired != 0).then_some(())?;
        Some(sum + u128::from(label.checked_sub(acquired)?))
    })?;
    u64::try_from(sum.checked_div(u128::from(frames))?).ok()
}

/// Coherent individual tap snapshots. Host intervals and queue passages are ticks.
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencySnapshot {
    pub faults: u64,
    pub acquisition: ResidenceSnapshot,
    pub presentation: ResidenceSnapshot,
    /// Window scalar means are fixed-point (1e9 = nominal); no valid sample means nominal.
    pub input_scalar: ResidenceSnapshot,
    pub output_scalar: ResidenceSnapshot,
    pub native_queue: PassageSnapshot,
    pub graph_queue: PassageSnapshot,
    /// Exact capture-to-graph mean in nanoseconds, including SRC and retained queues.
    pub content_age: ResidenceSnapshot,
}

/// Reads atomics only; a busy writer returns no snapshot instead of blocking.
/// # Safety
/// The native owner is retained throughout this call.
pub unsafe fn latency_snapshot(io: *const DeviceIo, direct: bool) -> Option<LatencySnapshot> {
    let io = unsafe { io.as_ref() }?;
    let taps = &io.measurements;
    let taps = [
        if direct {
            &taps.direct_acquisition
        } else {
            &taps.acquisition
        },
        if direct {
            &taps.direct_presentation
        } else {
            &taps.presentation
        },
        if direct {
            &taps.direct_input_scalar
        } else {
            &taps.input_scalar
        },
        if direct {
            &taps.direct_output_scalar
        } else {
            &taps.output_scalar
        },
    ]
    .map(Residence::snapshot);
    let queues =
        [&io.measurements.native_queue, &io.measurements.graph_queue].map(Passage::snapshot);
    let mut snapshot =
        coherent_snapshot(io.measurements.faults.load(Ordering::Acquire), taps, queues)?;
    snapshot.content_age = io.measurements.content_age.snapshot()?;
    Some(snapshot)
}

fn coherent_snapshot(
    faults: u64,
    taps: [Option<ResidenceSnapshot>; 4],
    queues: [Option<PassageSnapshot>; 2],
) -> Option<LatencySnapshot> {
    let (
        [
            Some(acquisition),
            Some(presentation),
            Some(input_scalar),
            Some(output_scalar),
        ],
        [Some(native_queue), Some(graph_queue)],
    ) = (taps, queues)
    else {
        return None;
    };
    Some(LatencySnapshot {
        faults,
        acquisition,
        presentation,
        input_scalar,
        output_scalar,
        native_queue,
        graph_queue,
        content_age: ResidenceSnapshot::default(),
    })
}

/// Same `IOProc` timings for the bindings actually rendered by the direct master. A
/// quantum rendered in several graph cycles records each cycle's `frames` once, so the
/// callback's total weight is its frame count.
/// # Safety
/// Direct rendering serializes this tap's sole writer; IO is retained until drain.
pub unsafe fn record_direct_latency(io: *const DeviceIo, timing: IoTiming, frames: u32) {
    if let Some(io) = unsafe { io.as_ref() } {
        io.measurements.callback(timing, frames, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_io::IoTimestamp;
    mod content_age;
    #[test]
    fn any_contended_tap_returns_no_new_evidence() {
        let taps = [Some(ResidenceSnapshot::default()); 4];
        let queues = [Some(PassageSnapshot::default()); 2];
        assert!(coherent_snapshot(0, taps, queues).is_some());
        for index in 0..taps.len() {
            let mut contended = taps;
            contended[index] = None;
            assert!(coherent_snapshot(0, contended, queues).is_none());
        }
        for index in 0..queues.len() {
            let mut contended = queues;
            contended[index] = None;
            assert!(coherent_snapshot(0, taps, contended).is_none());
        }
    }

    #[test]
    fn scalar_capture_skips_invalid_values_and_keeps_directional_window_means() {
        let io = DeviceIo::for_test(c"unit", true, 1, 48_000).unwrap();
        let before = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
        let timing = |input, output| IoTiming {
            input: IoTimestamp {
                rate_scalar: input,
                flags: 4,
                ..Default::default()
            },
            output: IoTimestamp {
                rate_scalar: output,
                flags: 4,
                ..Default::default()
            },
            ..Default::default()
        };
        let allocations = allocation_counter::measure(|| {
            for invalid in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1000.0] {
                io.measurements
                    .callback(timing(invalid, invalid), 256, false);
            }
            io.measurements.callback(timing(1.000, 0.998), 256, false);
            io.measurements.callback(timing(1.002, 1.000), 256, false);
            unsafe { record_direct_latency(&raw const io, timing(1.001, 1.002), 256) };
        });
        assert_eq!(allocations.count_total, 0);
        let next = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
        assert_eq!(
            next.input_scalar.mean_since(before.input_scalar),
            Some(1_001_000_000.0)
        );
        assert_eq!(
            next.output_scalar.mean_since(before.output_scalar),
            Some(999_000_000.0)
        );
        let direct = unsafe { latency_snapshot(&raw const io, true) }.unwrap();
        assert_eq!(
            direct.input_scalar.mean_since(before.input_scalar),
            Some(1_001_000_000.0)
        );
        assert_eq!(
            direct.output_scalar.mean_since(before.output_scalar),
            Some(1_002_000_000.0)
        );
    }

    #[test]
    fn synthetic_host_intervals_validate_flags_direction_and_direct_taps() {
        let io = DeviceIo::for_test(c"unit", true, 1, 48_000).unwrap();
        let time = |host_time| IoTimestamp {
            host_time,
            flags: 2,
            ..Default::default()
        };
        let timing = IoTiming {
            now: time(100),
            input: time(40),
            output: time(130),
        };
        let before = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
        let allocations = allocation_counter::measure(|| {
            io.measurements.callback(timing, 32, false);
            unsafe { record_direct_latency(&raw const io, timing, 32) };
        });
        assert_eq!(allocations.count_total, 0);
        let after = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
        assert_eq!(after.acquisition.mean_since(before.acquisition), Some(60.0));
        assert_eq!(
            after.presentation.mean_since(before.presentation),
            Some(30.0)
        );
        assert_eq!(
            unsafe { latency_snapshot(&raw const io, true) }
                .unwrap()
                .acquisition
                .mean_since(ResidenceSnapshot::default()),
            Some(60.0)
        );
        for invalid in [
            IoTiming::default(),
            IoTiming {
                now: time(150),
                ..timing
            },
        ] {
            io.measurements.callback(invalid, 32, false);
        }
        assert_eq!(
            unsafe { latency_snapshot(&raw const io, false) }
                .unwrap()
                .presentation
                .mean_since(after.presentation),
            None
        );
        assert!(unsafe { latency_snapshot(std::ptr::null(), false) }.is_none());
        unsafe { record_direct_latency(std::ptr::null(), timing, 32) };
    }

    /// Host intervals average per frame like FIFO residence. Counting callbacks once
    /// each, a 64- and a 1024-frame callback would mean 544 ticks instead of 967.53.
    #[test]
    fn variable_quanta_weight_host_intervals_by_frames_once_per_direct_split() {
        let io = DeviceIo::for_test(c"unit", true, 1, 48_000).unwrap();
        let time = |host_time| IoTimestamp {
            host_time,
            flags: 2,
            ..Default::default()
        };
        // Each callback arrives one tick per frame after its first captured frame.
        let quantum = |first: u64, frames: u32| IoTiming {
            now: time(first + u64::from(frames)),
            input: time(first),
            output: time(first + 2 * u64::from(frames)),
        };
        let before = unsafe { latency_snapshot(&raw const io, false) }.unwrap();
        let allocations = allocation_counter::measure(|| {
            io.measurements.callback(quantum(1_000, 64), 64, false);
            io.measurements
                .callback(quantum(2_000, 1_024), 1_024, false);
            // The direct master renders its 1024-frame quantum in four graph cycles.
            unsafe { record_direct_latency(&raw const io, quantum(1_000, 64), 64) };
            for _ in 0..4 {
                unsafe { record_direct_latency(&raw const io, quantum(2_000, 1_024), 256) };
            }
        });
        assert_eq!(allocations.count_total, 0);
        let frame_weighted = (64.0 * 64.0 + 1_024.0 * 1_024.0) / 1_088.0;
        for direct in [false, true] {
            let after = unsafe { latency_snapshot(&raw const io, direct) }.unwrap();
            assert_eq!(after.acquisition.weight, 1_088);
            assert_eq!(
                after.acquisition.mean_since(before.acquisition),
                Some(frame_weighted)
            );
            assert_eq!(
                after.presentation.mean_since(before.presentation),
                Some(frame_weighted)
            );
        }
    }
}
