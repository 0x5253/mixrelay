//! Coordinates travel with native/SRC PCM using the same ring's absolute cursor.
use super::{DeviceIo, GraphRead};
use std::sync::atomic::Ordering;

/// Graph consumer's largest producer publication between consecutive graph cycles.
#[derive(Default)]
pub(super) struct CaptureCadence {
    previous: Option<(u64, u64)>,
    burst: u32,
}

/// Actual SRC publication burst in graph frames, including coalesced native callbacks.
/// # Safety
/// Retained native input owner; caller is the sole serialized graph consumer and
/// observes this once per graph cycle, with its current activity/native epoch.
pub unsafe fn capture_burst(io: *const DeviceIo, epoch: u64) -> u32 {
    // SAFETY: retained optional owner and serialized access to the graph's private cadence.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return 0;
    };
    let cadence = unsafe { &mut *io.capture_cadence.get() };
    let written = io.graph_ring.stats().written;
    if let Some((previous_epoch, previous)) = cadence.previous {
        if previous_epoch == epoch {
            cadence.burst = cadence.burst.max(
                u32::try_from(written.saturating_sub(previous))
                    .unwrap_or(u32::MAX)
                    .min(super::FIFO_FRAMES / 2),
            );
        } else {
            cadence.burst = 0;
        }
    }
    cadence.previous = Some((epoch, written));
    cadence.burst
}

/// Shared alignment reserve in graph frames; zero uses ordinary FIFO policy.
/// # Safety
/// The native owner remains retained during this atomic observation.
pub unsafe fn capture_window(io: *const DeviceIo) -> u32 {
    // SAFETY: caller retains the optional native owner; the field is atomic.
    unsafe { io.as_ref() }.map_or(0, |io| io.progress.capture_window.load(Ordering::Acquire))
}

/// Reserves bounded input history for the largest independent producer burst.
/// # Safety
/// The native owner remains retained; this changes no ring cursor or callback state.
pub unsafe fn set_capture_window(io: *const DeviceIo, frames: u32) {
    // SAFETY: caller retains the optional native owner; no callback cell is borrowed.
    if let Some(io) = unsafe { io.as_ref() } {
        io.progress
            .capture_window
            .store(frames.min(super::FIFO_FRAMES / 2), Ordering::Release);
    }
}

/// Native source period measured between hardware sample/host pairs.
/// # Safety
/// Native owner remains retained throughout this atomic observation.
pub unsafe fn frame_nanoseconds(io: *const DeviceIo) -> f64 {
    // SAFETY: retained optional owner, no control/callback state is borrowed.
    unsafe { io.as_ref() }.map_or(0.0, |io| {
        f64::from_bits(io.progress.frame_nanos.load(Ordering::Acquire))
    })
}

/// Latest graph-input coordinate with the shared alignment reserve retained.
/// # Safety
/// Native owner remains retained; no other consumer mutates its graph cursor.
pub unsafe fn input_time(io: *const DeviceIo, frames: u32) -> Option<u64> {
    // SAFETY: caller retains the optional native owner and serializes its consumer.
    let io = unsafe { io.as_ref() }?;
    if !io.selection.input || io.progress.invalid.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: this is the graph's serialized consumer. A reset must retire old
    // PCM before its metadata can seed the common acquisition cursor.
    let stats = unsafe {
        mixrelay_pcm_ring::consumer_stats(std::ptr::from_ref(&io.graph_ring).cast_mut())
    }?;
    let reserve = frames.max(io.progress.capture_window.load(Ordering::Acquire) / 2);
    let start = stats.written.checked_sub(u64::from(reserve))?;
    if start < stats.read {
        return None;
    }
    io.graph_times.at(start).filter(|time| *time != 0)
}

// SAFETY: retained input owner, sole serialized graph consumer, including epoch ACK.
pub(super) unsafe fn seek_input(io: &DeviceIo, frames: u32, coordinate: (u64, u32)) -> bool {
    let (nanos, rate) = coordinate;
    let ring = std::ptr::from_ref(&io.graph_ring).cast_mut();
    // SAFETY: this is the graph's sole consumer and no PCM reservation is live.
    let Some(stats) = (unsafe { mixrelay_pcm_ring::consumer_stats(ring) }) else {
        return false;
    };
    let Some(start) = io.graph_times.locate(stats.read, stats.written, nanos) else {
        return false;
    };
    let valid = start + u64::from(frames) <= stats.written
        && rate != 0
        && (0..frames).all(|offset| {
            let target = nanos.saturating_add(u64::from(offset) * 1_000_000_000 / u64::from(rate));
            io.graph_times
                .at(start + u64::from(offset))
                .is_some_and(|actual| {
                    actual != 0 && actual.abs_diff(target) <= 1_000_000_000 / u64::from(rate) + 2
                })
        });
    let Ok(skip) = u32::try_from(start - stats.read) else {
        return false;
    };
    // SAFETY: same serialized graph consumer; metadata selection moves only its cursor.
    valid && (skip == 0 || unsafe { mixrelay_pcm_ring::skip_available(ring, skip) })
}

/// Reads a graph input at its common acquisition coordinate.
/// # Safety
/// Same retained owner/serialized graph consumer contract as `graph_read`.
pub unsafe fn graph_read_at(
    io: *mut DeviceIo,
    samples: &mut [f32],
    epoch: u64,
    timing: (u64, u64, u32),
) -> GraphRead {
    samples.fill(0.0);
    // SAFETY: caller retains the optional native owner throughout this read.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return GraphRead::default();
    };
    if !samples.len().is_multiple_of(io.selection.channels as usize) {
        return GraphRead::default();
    }
    let Ok(frames) = u32::try_from(samples.len() / io.selection.channels as usize) else {
        return GraphRead::default();
    };
    // SAFETY: caller owns the full output and graph cursor; timing carries no pointers.
    unsafe {
        super::queues::graph_read(
            io,
            samples.as_mut_ptr(),
            frames,
            epoch,
            (timing.0, Some((timing.1, timing.2))),
        )
    }
}

/// Reads the exact native/SRC input coordinates alongside its PCM.
/// # Safety
/// Retained owner, sole bridge consumer, complete disjoint PCM/time output extents.
pub unsafe fn bridge_read_timed(io: *mut DeviceIo, samples: &mut [f32], times: &mut [u64]) -> u32 {
    // SAFETY: caller retains the optional native owner and owns this bridge consumer.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return 0;
    };
    if samples.len() != times.len() * io.selection.channels as usize {
        return 0;
    }
    let Ok(frames) = u32::try_from(times.len()) else {
        return 0;
    };
    // SAFETY: this callback owns the bridge consumer and both writable extents.
    let graph = !io.selection.input;
    let stamps = if graph {
        &io.graph_times
    } else {
        &io.native_times
    };
    let mut valid = true;
    let read = unsafe {
        super::queues::bridge_read_observed(io, samples.as_mut_ptr(), frames, |cursor| {
            valid = stamps.read(cursor, times);
        })
    };
    if !valid {
        times[..read as usize].fill(0);
        samples[..read as usize * io.selection.channels as usize].fill(0.0);
    }
    read
}

/// Publishes SRC output coordinates before releasing the matching PCM block.
/// # Safety
/// Retained owner, sole bridge producer, complete immutable PCM/time input extents.
pub unsafe fn bridge_write_timed(io: *mut DeviceIo, samples: &[f32], times: &[u64]) -> u32 {
    // SAFETY: caller retains the optional native owner and owns this bridge producer.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return 0;
    };
    if samples.len() != times.len() * io.selection.channels as usize {
        return 0;
    }
    let Ok(frames) = u32::try_from(times.len()) else {
        return 0;
    };
    let graph = io.selection.input;
    let stamps = if graph {
        &io.graph_times
    } else {
        &io.native_times
    };
    // SAFETY: the same producer owns this exact PCM range; coordinates already precede it.
    unsafe {
        super::queues::bridge_write_observed(io, samples.as_ptr(), frames, |cursor| {
            stamps.publish(cursor, times);
        })
    }
}
