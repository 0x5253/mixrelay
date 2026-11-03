use super::{AudioRing, allocate_samples, create};
use std::{
    alloc::alloc,
    ptr::{self, NonNull},
    slice,
    sync::atomic::Ordering,
};

/// Publishes the source coordinates before the matching PCM release.
/// # Safety
/// Same sole-producer and complete input extent contract as the ordinary write.
pub unsafe fn write_timed_interleaved(
    ring: *mut AudioRing,
    input: *const f32,
    frames: usize,
    first_nanos: u64,
    frame_nanos: f64,
) -> usize {
    // SAFETY: the retained ring and serialized producer are caller-owned.
    let Some(owner) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    let Some(cursor) = owner.reserve(frames) else {
        return 0;
    };
    owner.times.range(cursor, frames, first_nanos, frame_nanos);
    // SAFETY: same input and ownership, metadata precedes PCM publication.
    unsafe { mr_audio_ring_write_interleaved(ring, input, frames) }
}

/// Planar equivalent of [`write_timed_interleaved`].
/// # Safety
/// Same sole-producer and readable plane extents as the ordinary planar write.
pub unsafe fn write_timed_planar(
    ring: *mut AudioRing,
    input: *const *const f32,
    frames: usize,
    first_nanos: u64,
    frame_nanos: f64,
) -> usize {
    // SAFETY: the retained ring and serialized producer are caller-owned.
    let Some(owner) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    let Some(cursor) = owner.reserve(frames) else {
        return 0;
    };
    owner.times.range(cursor, frames, first_nanos, frame_nanos);
    // SAFETY: same input planes and ownership, metadata precedes PCM publication.
    unsafe { mr_audio_ring_write_planar(ring, input, frames) }
}

/// Reads PCM and the coordinates of the same absolute ring positions.
/// # Safety
/// The sole consumer owns output for maximum*channels samples and maximum timestamps.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_read_timed(
    ring: *mut AudioRing,
    output: *mut f32,
    times: *mut u64,
    maximum: usize,
) -> usize {
    if times.is_null() || output.is_null() {
        return 0;
    }
    // SAFETY: owner remains retained and this is the sole consumer.
    let Some(owner) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    let (cursor, frames) = owner.read_window(maximum);
    // Read coordinates while the consumer cursor still protects their PCM slots.
    let coordinates = unsafe { slice::from_raw_parts_mut(times, frames) };
    let valid = owner.times.read(cursor, coordinates);
    // SAFETY: the exact acquired frame range cannot shrink or be reused before this read.
    let read = unsafe { mr_audio_ring_read_interleaved(ring, output, frames) };
    if !valid {
        coordinates.fill(0);
        // SAFETY: successfully copied output remains caller-owned.
        unsafe { ptr::write_bytes(output, 0, read * owner.channels) };
    }
    read
}

/// Allocates an app-capture ring for power-of-two capacity 1..=2^20 and 1..=64
/// channels. Invalid geometry or either allocation failure returns null.
#[unsafe(no_mangle)]
pub extern "C" fn mr_audio_ring_create(capacity: usize, channels: u32) -> *mut AudioRing {
    // SAFETY: std alloc returns fresh aligned storage or null; create initializes
    // the owner before returning it to MRProcessTap's control-side setup.
    unsafe {
        create(capacity, channels, allocate_samples, |layout| {
            NonNull::new(alloc(layout))
        })
    }
}

/// Releases both the PCM slab and native owner. Null is a no-op.
/// # Safety
/// `MRProcessTap` owns this create result exactly once. `IOProc` unregistration and
/// pump/stat-reader drain must complete before destroy; no other access may remain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_destroy(ring: *mut AudioRing) {
    if !ring.is_null() {
        // SAFETY: create used the global allocator with precisely AudioRing's
        // layout; the native control owner has recovered exclusive ownership.
        drop(unsafe { Box::from_raw(ring) });
    }
}

/// Writes the complete block or rejects it, counting capacity-rejected frames.
/// Null input/ring and zero frames return zero without changing counters.
/// # Safety
/// Ring is a live create result retained by `MRProcessTap`. Its serialized `IOProc`
/// is the sole producer. For an accepted block, input contains frames*channels
/// readable floats, disjoint from ring storage, until this call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_write_interleaved(
    ring: *mut AudioRing,
    input: *const f32,
    frames: usize,
) -> usize {
    // SAFETY: native owner retains the ring; mutation is confined to cells/atomics.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if input.is_null() {
        return 0;
    }
    let Some(written) = ring.reserve(frames) else {
        return 0;
    };
    let (start, first) = ring.split(written, frames);
    // UnsafeCell has f32's layout. Never borrow the whole slab mutably: the pump
    // may concurrently read a disjoint range protected by the consumer cursor.
    let destination = ring.samples.as_ptr().cast::<f32>().cast_mut();
    // SAFETY: reserve's acquire sees released read ranges; sole producer owns
    // exactly these bounded free spans until the release publication below.
    unsafe {
        ptr::copy_nonoverlapping(input, destination.add(start), first);
        if first < frames * ring.channels {
            ptr::copy_nonoverlapping(
                input.add(first),
                destination,
                frames * ring.channels - first,
            );
        }
    }
    ring.publish(written, frames);
    frames
}

/// Interleaves a complete planar block. A null channel rejects/counts the block
/// once, without publishing partial PCM; null outer input is not counted.
/// # Safety
/// Ring and producer ownership match `write_interleaved`. Non-null input addresses
/// channels readable pointers; each non-null channel has frames readable floats
/// for an accepted block. All input storage is disjoint from ring storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_write_planar(
    ring: *mut AudioRing,
    input: *const *const f32,
    frames: usize,
) -> usize {
    // SAFETY: initialized ring retained by the native owner, sole producer call.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if input.is_null() {
        return 0;
    }
    let Some(written) = ring.reserve(frames) else {
        return 0;
    };
    // SAFETY: native input table contains exactly the ring's bounded channel width.
    let planes = unsafe { slice::from_raw_parts(input, ring.channels) };
    if planes.iter().any(|plane| plane.is_null()) {
        ring.drop_frames(frames);
        return 0;
    }
    let (start, first_samples) = ring.split(written, frames);
    let first_frames = first_samples / ring.channels;
    let destination = ring.samples.as_ptr().cast::<f32>().cast_mut();
    // SAFETY: reserve owns both free spans, with validated readable planes.
    // Split once per block, rather than recomputing ring wrap for every frame.
    unsafe {
        interleave_span(planes, 0, destination.add(start), first_frames);
        interleave_span(planes, first_frames, destination, frames - first_frames);
    }
    ring.publish(written, frames);
    frames
}

// SAFETY: destination is one exclusively producer-owned span of frames*channels
// cells. Each plane is readable through source_start+frames and disjoint from it.
unsafe fn interleave_span(
    planes: &[*const f32],
    source_start: usize,
    destination: *mut f32,
    frames: usize,
) {
    // The C compiler unrolled channel loads; Rust's dynamic inner loop did not.
    // Fixed-size groups retain that bounded copy throughput without OS SIMD code.
    let (groups, tail) = planes.as_chunks::<8>();
    for frame in 0..frames {
        for (group_index, group) in groups.iter().enumerate() {
            for (channel, plane) in group.iter().enumerate() {
                // SAFETY: complete channel group within the reserved span.
                unsafe {
                    destination
                        .add(frame * planes.len() + group_index * 8 + channel)
                        .write(*plane.add(source_start + frame));
                }
            }
        }
        for (channel, plane) in tail.iter().enumerate() {
            // SAFETY: both offsets are within the accepted source/destination spans.
            unsafe {
                destination
                    .add(frame * planes.len() + groups.len() * 8 + channel)
                    .write(*plane.add(source_start + frame));
            }
        }
    }
}

/// Reads up to maximum frames immediately; the remaining destination is untouched.
/// # Safety
/// Ring is retained while the pump's single consumer owns this call. Non-null
/// destination is exclusively writable for the returned frames*channels floats,
/// disjoint from ring storage. Refresh drains only after the `IOProc` unregisters
/// and under the same consumer ownership; a second consumer is not permitted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_read_interleaved(
    ring: *mut AudioRing,
    destination: *mut f32,
    maximum: usize,
) -> usize {
    // SAFETY: initialized owner-retained ring, only shared immutable/atomic access.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if destination.is_null() || maximum == 0 {
        return 0;
    }
    let (read, frames) = ring.read_window(maximum);
    if frames == 0 {
        return 0;
    }
    let (start, first) = ring.split(read, frames);
    let source = ring.samples.as_ptr().cast::<f32>();
    // SAFETY: acquire observed the producer's completed write. These published
    // cells cannot be reused until this sole consumer releases its cursor.
    unsafe {
        ptr::copy_nonoverlapping(source.add(start), destination, first);
        if first < frames * ring.channels {
            ptr::copy_nonoverlapping(
                source,
                destination.add(first),
                frames * ring.channels - first,
            );
        }
    }
    ring.read
        .store(read.wrapping_add(frames as u64), Ordering::Release);
    frames
}

/// Returns immutable capacity, or zero for null.
/// # Safety
/// Non-null ring is initialized and retained by the tap/pump owner during the read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_capacity_frames(ring: *const AudioRing) -> usize {
    // SAFETY: retained initialized owner; immutable geometry never changes.
    unsafe { ring.as_ref() }.map_or(0, |ring| ring.capacity)
}

/// Returns a capacity-bounded observation, not a transactional cursor snapshot.
/// # Safety
/// Non-null ring is initialized and retained by the tap/pump owner during the read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_available_frames(ring: *const AudioRing) -> usize {
    // SAFETY: retained owner; statistics read atomics and never access PCM cells.
    unsafe { ring.as_ref() }.map_or(0, |ring| ring.read_window(ring.capacity).1)
}

/// Returns cumulative rejected frames, wrapping at u64 exhaustion; null returns zero.
/// # Safety
/// Non-null ring is initialized and retained by the tap/pump owner during the read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_audio_ring_dropped_frames(ring: *const AudioRing) -> u64 {
    // SAFETY: retained owner; producer updates this counter atomically.
    unsafe { ring.as_ref() }.map_or(0, |ring| ring.dropped.load(Ordering::Relaxed))
}
