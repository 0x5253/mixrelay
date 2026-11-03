use super::{DeviceIo, MAX_FRAMES};
use mixrelay_pcm_ring::{mr_pcm_ring_discard, mr_pcm_ring_write};
use std::{
    ptr,
    sync::atomic::Ordering::{Acquire, Relaxed, Release},
};

/// Actual graph-input transfer and one-time admission of its consumer timeline.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GraphRead {
    /// Valid PCM frames written to the caller's output, never synthetic priming.
    pub frames: u32,
    /// True only for the first nonempty read request of this native owner.
    /// Subsequent shortages and recovery epochs are not initial admission.
    pub initial_epoch: bool,
    /// A previously admitted input changed its PCM epoch. Even an empty first
    /// read after recovery must reset downstream state before fresh audio flows.
    pub discontinuity: bool,
}

const _: () = assert!(size_of::<GraphRead>() == 8 && align_of::<GraphRead>() == 4);

// SAFETY: graph rendering is serialized; its output span is complete and disjoint
// from either ring. The SRC owner owns only the opposite producer cursor.
pub(super) unsafe fn graph_read(
    io: &DeviceIo,
    samples: *mut f32,
    frames: u32,
    epoch: u64,
    timing: (u64, Option<(u64, u32)>),
) -> GraphRead {
    let (host, requested) = timing;
    if io.progress.invalid.load(Acquire)
        || !io.selection.input
        || frames == 0
        || frames > MAX_FRAMES
    {
        return GraphRead::default();
    }
    graph_progress(io, frames);
    let ring = ptr::from_ref(&io.graph_ring).cast_mut();
    // SAFETY: only the serialized graph consumer accesses this private epoch cell.
    let previous = unsafe { &mut *io.graph_epoch.get() };
    let initial_epoch = previous.is_none();
    let discontinuity = previous.is_some_and(|previous| previous != epoch);
    if *previous != Some(epoch) {
        // SAFETY: discard changes only this graph consumer's cursor.
        // Timestamped reads select within the PCM ring's acknowledged producer
        // epoch. Discarding here would also erase fresh recovery coordinates.
        if requested.is_none() {
            unsafe { mr_pcm_ring_discard(ring) };
        }
        *previous = Some(epoch);
        io.measurements.faults.fetch_add(1, Release);
    }
    if let Some(coordinate) = requested
        // SAFETY: this call is the sole graph consumer, including epoch ACK and seek.
        && !unsafe { super::timestamps::seek_input(io, frames, coordinate) }
    {
        io.measurements.faults.fetch_add(1, Release);
        // SAFETY: graph owns the full writable PCM extent.
        unsafe {
            ptr::write_bytes(samples, 0, frames as usize * io.selection.channels as usize);
        };
        return GraphRead {
            frames: 0,
            initial_epoch,
            discontinuity,
        };
    }
    // SAFETY: retained ring and sole graph consumer, complete caller-owned output.
    let mut content_age = None;
    let read = unsafe {
        mixrelay_pcm_ring::read_available_observed(ring, samples, frames, |cursor| {
            content_age = super::latency::capture_age(io, cursor, frames, host);
        })
    };
    if read != 0 {
        io.measurements
            .content_age
            .record(content_age, u64::from(read));
    }
    if read < frames {
        io.measurements.faults.fetch_add(1, Release);
    }
    io.consumed(true, read, host);
    let frames = read;
    GraphRead {
        frames,
        initial_epoch,
        discontinuity,
    }
}

// SAFETY: one serialized graph producer; immutable input spans frames*channels floats.
pub(super) unsafe fn graph_write(
    io: &DeviceIo,
    samples: *const f32,
    frames: u32,
    host: u64,
) -> u32 {
    if io.progress.invalid.load(Acquire) || io.selection.input || frames > MAX_FRAMES {
        return 0;
    }
    graph_progress(io, frames);
    // SAFETY: graph is this ring's sole producer; native callbacks use the other ring.
    let written =
        unsafe { mr_pcm_ring_write(ptr::from_ref(&io.graph_ring).cast_mut(), samples, frames) };
    io.produced(true, written, host);
    written
}

fn graph_progress(io: &DeviceIo, frames: u32) {
    io.progress.graph_quantum.store(frames, Relaxed);
    io.progress
        .graph_frames
        .fetch_add(u64::from(frames), Release);
}

// SAFETY: the SRC owner is the selected ring's sole consumer, and its output
// span owns frames*channels writable floats even when no PCM is available.
// It runs in the IOProc's PCM hook, so its frames pass at that callback's host time.
pub(super) unsafe fn bridge_read(io: &DeviceIo, samples: *mut f32, frames: u32) -> u32 {
    // SAFETY: same retained owner, exclusive output and sole consumer.
    unsafe { bridge_read_observed(io, samples, frames, |_| {}) }
}

// SAFETY: same contract as bridge_read; observer only reads this range's metadata.
pub(super) unsafe fn bridge_read_observed(
    io: &DeviceIo,
    samples: *mut f32,
    frames: u32,
    observe: impl FnOnce(u64),
) -> u32 {
    if frames == 0 || frames > MAX_FRAMES {
        return 0;
    }
    // No prefill: SRC owns priming, not a second native-queue latency policy.
    // SAFETY: the bridge owner's input ring depends only on immutable direction.
    let read = unsafe {
        mixrelay_pcm_ring::read_available_observed(
            ptr::from_ref(io.ring(!io.selection.input)).cast_mut(),
            samples,
            frames,
            observe,
        )
    };
    io.consumed(
        !io.selection.input,
        read,
        io.progress.callback_host.load(Relaxed),
    );
    read
}

// SAFETY: sole SRC owner producer, full immutable PCM disjoint from queue storage.
pub(super) unsafe fn bridge_write(io: &DeviceIo, samples: *const f32, frames: u32) -> u32 {
    unsafe { bridge_write_observed(io, samples, frames, |_| {}) }
}

// SAFETY: same producer/PCM contract; observer publishes only metadata of the reserved range.
pub(super) unsafe fn bridge_write_observed(
    io: &DeviceIo,
    samples: *const f32,
    frames: u32,
    observe: impl FnOnce(u64),
) -> u32 {
    if frames > MAX_FRAMES {
        return 0;
    }
    // SAFETY: bridge owner writes graph input or native output; no other role writes that ring.
    let written = unsafe {
        mixrelay_pcm_ring::write_observed(
            ptr::from_ref(io.ring(io.selection.input)).cast_mut(),
            samples,
            frames,
            observe,
        )
    };
    io.produced(
        io.selection.input,
        written,
        io.progress.callback_host.load(Relaxed),
    );
    written
}

// SAFETY: sole bridge consumer; the opposite producer may run concurrently.
pub(super) unsafe fn bridge_discard(io: &DeviceIo) {
    // SAFETY: only the bridge owner input ring's consumer cursor is changed.
    unsafe { mr_pcm_ring_discard(ptr::from_ref(io.ring(!io.selection.input)).cast_mut()) };
    if io.selection.input {
        // SAFETY: this callback is the graph-input ring's sole producer. Publish
        // the new epoch even when old PCM fills the queue and pump cannot write.
        // Only its graph consumer will retire the old cursor.
        let ring = ptr::from_ref(&io.graph_ring).cast_mut();
        unsafe {
            mixrelay_pcm_ring::mr_pcm_ring_restart(ring);
            mr_pcm_ring_write(ring, ptr::null(), 0);
        }
    }
}
