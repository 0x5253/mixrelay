//! Pointer-free, interprocess Float32 transport; all storage is caller-owned.
//!
//! Exactly one PCM writer publishes monotonically increasing absolute frames in
//! each epoch. Readers may run concurrently in other processes. Header geometry
//! is immutable after exclusive initialization; every mutable shared cell is an
//! aligned native atomic. No callback operation allocates, spins or waits.
//!
//! The current wire schema includes an epoch-tagged publication watermark and a
//! bounded producer claim. Attach accepts only this exact schema; mappings are
//! replaced, never reinitialized while another owner may still access them.

use std::{
    ffi::c_void,
    mem::{align_of, size_of},
    ptr,
    sync::atomic::{AtomicU32, AtomicU64, Ordering, fence},
};

const MAGIC: u64 = 0x4d52_4155_4449_4f31;
const VERSION: u32 = 4;
const MAX_CHANNELS: u32 = 64;
const MAX_CAPACITY: u32 = 1 << 20;
const FRAME_BITS: u32 = 48;
const FRAME_MASK: u64 = (1 << FRAME_BITS) - 1;
const MAX_EPOCH: u64 = (1 << 16) - 1;
const REQUESTED: u64 = 1 << 63;
const RETIRED: u64 = 1 << 62;

#[repr(C)]
pub struct SharedAudioBuffer {
    magic: u64,
    version: u32,
    total_bytes: u32,
    capacity_frames: u32,
    capacity_mask: u32,
    channels: u32,
    producer_activity: AtomicU32,
    epoch: AtomicU64,
    written_frames: AtomicU64,
    read_frames: AtomicU64,
    missing_frames: AtomicU64,
    // High 16 bits: generation. Low 48 bits: exclusive published frame end.
    published_through_frame: AtomicU64,
    producer_busy: AtomicU32,
    reserved: u32,
}

const _: () = assert!(size_of::<SharedAudioBuffer>() == 80);
const _: () = assert!(std::mem::offset_of!(SharedAudioBuffer, producer_activity) == 28);
const _: () = assert!(std::mem::offset_of!(SharedAudioBuffer, epoch) == 32);
const _: () = assert!(std::mem::offset_of!(SharedAudioBuffer, producer_busy) == 72);
const _: () = assert!(align_of::<SharedAudioBuffer>() == 8);
const _: () = assert!(cfg!(target_has_atomic = "64") && cfg!(target_has_atomic = "32"));

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SharedAudioBufferStats {
    pub written_frames: u64,
    pub read_frames: u64,
    pub missing_frames: u64,
    pub epoch: u64,
    pub published_through_frame: u64,
}

fn required_bytes(capacity: u32, channels: u32) -> Option<usize> {
    if !capacity.is_power_of_two()
        || capacity > MAX_CAPACITY
        || !(1..=MAX_CHANNELS).contains(&channels)
    {
        return None;
    }
    let capacity = usize::try_from(capacity).ok()?;
    let channels = usize::try_from(channels).ok()?;
    size_of::<SharedAudioBuffer>()
        .checked_add(capacity.checked_mul(size_of::<AtomicU64>())?)?
        .checked_add(
            capacity
                .checked_mul(channels)?
                .checked_mul(size_of::<AtomicU32>())?,
        )
        .filter(|size| u32::try_from(*size).is_ok())
}

fn active(epoch: u64) -> bool {
    (1..=MAX_EPOCH).contains(&epoch)
}
fn range_end(start: u64, count: u32) -> Option<u64> {
    // The exclusive end itself must fit the packed 48-bit watermark. Therefore
    // FRAME_MASK is a valid end, not a publishable final sample-frame index.
    if count == 0 {
        Some(start)
    } else {
        start
            .checked_add(u64::from(count))
            .filter(|end| *end <= FRAME_MASK)
    }
}
fn stamp(epoch: u64, frame: u64) -> u64 {
    (epoch << FRAME_BITS) | frame
}

/// Serializes PCM publication and epoch advancement with one bounded CAS. A
/// callback never waits for another producer; the owner releases on every exit.
struct ProducerClaim<'a>(&'a AtomicU32);

impl Drop for ProducerClaim<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

impl SharedAudioBuffer {
    fn claim_producer(&self) -> Option<ProducerClaim<'_>> {
        self.producer_busy
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| ProducerClaim(&self.producer_busy))
    }
    unsafe fn stamp_cell(&self, slot: usize) -> &AtomicU64 {
        // SAFETY: caller supplied a validated live mapping. Header size is a
        // multiple of atomic alignment and slot is masked by immutable capacity.
        let base = unsafe {
            ptr::from_ref(self)
                .cast::<u64>()
                .add(size_of::<Self>() / size_of::<u64>())
                .add(slot)
        };
        unsafe { AtomicU64::from_ptr(base.cast_mut()) }
    }
    unsafe fn sample_cell(&self, sample: usize) -> &AtomicU32 {
        let offset = size_of::<Self>() + self.capacity_frames as usize * size_of::<AtomicU64>();
        // SAFETY: validated geometry places this sample inside the same mapping.
        let base = unsafe {
            ptr::from_ref(self)
                .cast::<u32>()
                .add(offset / size_of::<u32>())
                .add(sample)
        };
        unsafe { AtomicU32::from_ptr(base.cast_mut()) }
    }
    fn publication(&self) -> (u64, u64) {
        let before = self.epoch.load(Ordering::Acquire);
        if !active(before) {
            return (0, 0);
        }
        let published = self.published_through_frame.load(Ordering::Acquire);
        let after = self.epoch.load(Ordering::Acquire);
        if before != after {
            return (0, 0);
        }
        let end = if published >> FRAME_BITS == before {
            published & FRAME_MASK
        } else {
            0
        };
        (before, end)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mr_shared_audio_buffer_required_bytes(
    capacity_frames: u32,
    channels: u32,
) -> usize {
    required_bytes(capacity_frames, channels).unwrap_or(0)
}

/// Initializes a new mapping without allocation or embedded process pointers.
/// # Safety
/// `memory` must own `memory_size` writable bytes exclusively for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_initialize(
    memory: *mut c_void,
    memory_size: usize,
    capacity_frames: u32,
    channels: u32,
) -> *mut SharedAudioBuffer {
    // SAFETY: the caller's exclusive storage contract is forwarded unchanged.
    unsafe {
        initialize(
            memory,
            memory_size,
            capacity_frames,
            channels,
            Storage::Reused,
        )
    }
}

/// Initializes only the header of storage the kernel has just zero-filled, such as a
/// new anonymous mapping. Untouched ring pages stay unfaulted: a zero stamp never
/// matches an active epoch, so every frame reads as missing until a writer commits it.
/// # Safety
/// `memory` must own `memory_size` writable bytes exclusively for this call, and every
/// byte must still be zero. Reused or caller-recycled storage must use
/// [`mr_shared_audio_buffer_initialize`].
pub unsafe fn initialize_zeroed(
    memory: *mut c_void,
    memory_size: usize,
    capacity_frames: u32,
    channels: u32,
) -> *mut SharedAudioBuffer {
    // SAFETY: the caller guarantees exclusive, still-zero storage.
    unsafe {
        initialize(
            memory,
            memory_size,
            capacity_frames,
            channels,
            Storage::Zeroed,
        )
    }
}

/// Whether stamp and PCM cells may hold bytes from a previous user of the storage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Storage {
    Reused,
    Zeroed,
}

// SAFETY: memory owns memory_size exclusive writable bytes; Zeroed storage is all zero.
unsafe fn initialize(
    memory: *mut c_void,
    memory_size: usize,
    capacity_frames: u32,
    channels: u32,
    storage: Storage,
) -> *mut SharedAudioBuffer {
    let Some(bytes) = required_bytes(capacity_frames, channels) else {
        return ptr::null_mut();
    };
    if memory.is_null()
        || !(memory as usize).is_multiple_of(align_of::<SharedAudioBuffer>())
        || memory_size < bytes
    {
        return ptr::null_mut();
    }
    let Ok(total_bytes) = u32::try_from(bytes) else {
        return ptr::null_mut();
    };
    let buffer = memory.cast::<SharedAudioBuffer>();
    // SAFETY: exclusive, suitably aligned caller storage was extent-checked.
    // Zero is a valid bit pattern for every trailing atomic PCM/stamp cell, and a
    // zero stamp is never a committed frame. Only reused storage needs clearing.
    unsafe {
        if storage == Storage::Reused {
            ptr::write_bytes(memory.cast::<u8>(), 0, bytes);
        }
        buffer.write(SharedAudioBuffer {
            magic: MAGIC,
            version: VERSION,
            total_bytes,
            capacity_frames,
            capacity_mask: capacity_frames - 1,
            channels,
            producer_activity: AtomicU32::new(0),
            epoch: AtomicU64::new(1),
            written_frames: AtomicU64::new(0),
            read_frames: AtomicU64::new(0),
            missing_frames: AtomicU64::new(0),
            published_through_frame: AtomicU64::new(0),
            producer_busy: AtomicU32::new(0),
            reserved: 0,
        });
    }
    buffer
}

/// Attaches only compatible immutable geometry; no shared state is modified.
/// # Safety
/// The initialized mapping must remain accessible and its geometry immutable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_attach(
    memory: *mut c_void,
    memory_size: usize,
) -> *mut SharedAudioBuffer {
    if memory.is_null()
        || !(memory as usize).is_multiple_of(align_of::<SharedAudioBuffer>())
        || memory_size < size_of::<SharedAudioBuffer>()
    {
        return ptr::null_mut();
    }
    let buffer = memory.cast::<SharedAudioBuffer>();
    let header = unsafe { &*buffer };
    let Some(bytes) = required_bytes(header.capacity_frames, header.channels) else {
        return ptr::null_mut();
    };
    if header.magic != MAGIC
        || header.version != VERSION
        || header.capacity_mask != header.capacity_frames - 1
        || header.total_bytes as usize != bytes
        || memory_size < bytes
    {
        return ptr::null_mut();
    }
    buffer
}

/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_is_lock_free(
    buffer: *const SharedAudioBuffer,
) -> bool {
    // Rust's native AtomicU32/U64 implementations are lock-free on supported
    // targets; compilation above rejects a target without either atomic width.
    !buffer.is_null()
}

/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_capacity(buffer: *const SharedAudioBuffer) -> u32 {
    unsafe { buffer.as_ref() }.map_or(0, |buffer| buffer.capacity_frames)
}
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_channels(buffer: *const SharedAudioBuffer) -> u32 {
    unsafe { buffer.as_ref() }.map_or(0, |buffer| buffer.channels)
}
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_set_producer_activity(
    buffer: *mut SharedAudioBuffer,
    activity: u32,
) {
    if let Some(buffer) = unsafe { buffer.as_ref() } {
        buffer.producer_activity.store(activity, Ordering::Release);
    }
}
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_producer_activity(
    buffer: *const SharedAudioBuffer,
) -> u32 {
    unsafe { buffer.as_ref() }.map_or(0, |buffer| {
        let value = buffer.producer_activity.load(Ordering::Acquire);
        if value <= 3 { value } else { 0 }
    })
}
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_published_through(
    buffer: *const SharedAudioBuffer,
) -> u64 {
    unsafe { buffer.as_ref() }.map_or(0, |buffer| buffer.publication().1)
}

/// Bounded reader-side rearm request. It never clears storage or consumes a generation.
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_request_epoch(
    buffer: *mut SharedAudioBuffer,
) -> u64 {
    unsafe { buffer.as_ref() }.map_or(0, |buffer| {
        buffer.epoch.fetch_or(REQUESTED, Ordering::AcqRel) & MAX_EPOCH
    })
}
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_epoch_pending(
    buffer: *const SharedAudioBuffer,
) -> bool {
    unsafe { buffer.as_ref() }
        .is_some_and(|buffer| buffer.epoch.load(Ordering::Acquire) & REQUESTED != 0)
}

/// Distinguishes exhausted/retired storage from a normal callback rearm request.
/// Ordinary pending epochs can advance; a retired producer cannot be revived.
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_requires_replacement(
    buffer: *const SharedAudioBuffer,
) -> bool {
    unsafe { buffer.as_ref() }.is_some_and(|buffer| {
        let epoch = buffer.epoch.load(Ordering::Acquire);
        epoch & RETIRED != 0 || epoch == (REQUESTED | MAX_EPOCH)
    })
}

/// Retires a claim that remains busy after its sole writer has been drained or
/// has exited. Never clears the claim or reuses its mapping: another process's
/// reader may still hold old samples, and an old writer may have died mid-frame.
/// # Safety
/// Non-null buffer must be initialized and live. The caller must own the sole
/// writer lifecycle and prove all its callbacks/control producers have drained,
/// or that the predecessor writer process has exited. This must not be called
/// against a live remote writer merely because a callback observed contention.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_retire_busy_producer_after_drain(
    buffer: *mut SharedAudioBuffer,
) -> bool {
    let Some(buffer) = (unsafe { buffer.as_ref() }) else {
        return false;
    };
    if buffer.producer_busy.load(Ordering::Acquire) == 0 {
        return false;
    }
    buffer.epoch.fetch_or(REQUESTED | RETIRED, Ordering::AcqRel);
    true
}

/// Holds the producer claim as an in-flight writer callback does, for tests of callers
/// that must not mistake it for exhaustion.
/// # Safety
/// A non-null buffer must reference an initialized mapping that outlives the claim.
#[cfg(any(test, feature = "test-observation"))]
pub unsafe fn hold_producer<'a>(buffer: *const SharedAudioBuffer) -> Option<impl Drop + 'a> {
    unsafe { buffer.as_ref::<'a>() }.and_then(SharedAudioBuffer::claim_producer)
}

/// Whether the ring can never open another epoch: retired, or its last generation used.
/// A producer claim held by an in-flight writer is transient and does not count.
/// # Safety
/// A non-null buffer must reference an initialized, attached, live mapping.
pub unsafe fn epoch_exhausted(buffer: *const SharedAudioBuffer) -> bool {
    unsafe { buffer.as_ref() }.is_none_or(|buffer| {
        let epoch = buffer.epoch.load(Ordering::Acquire);
        epoch & !(REQUESTED | MAX_EPOCH) != 0 || !(1..MAX_EPOCH).contains(&(epoch & MAX_EPOCH))
    })
}

/// Whether `mr_shared_audio_buffer_try_begin_epoch` would open another epoch now,
/// without claiming the producer or changing any shared state.
/// # Safety
/// A non-null buffer must reference an initialized, attached, live mapping.
pub unsafe fn can_begin_epoch(buffer: *const SharedAudioBuffer) -> bool {
    unsafe { buffer.as_ref() }.is_some_and(|buffer| {
        let epoch = buffer.epoch.load(Ordering::Acquire);
        let generation = epoch & MAX_EPOCH;
        buffer.producer_busy.load(Ordering::Acquire) == 0
            && epoch & !(REQUESTED | MAX_EPOCH) == 0
            && (1..MAX_EPOCH).contains(&generation)
    })
}

/// One producer claim and one epoch CAS, with no retry or storage clearing.
/// # Safety
/// Non-null pointers must reference an initialized mapping. One PCM writer owns
/// publication; epoch invalidation may race readers and control-thread requests.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_try_begin_epoch(
    buffer: *mut SharedAudioBuffer,
) -> u64 {
    let Some(buffer) = (unsafe { buffer.as_ref() }) else {
        return 0;
    };
    let Some(_producer) = buffer.claim_producer() else {
        // Invalidate immediately without clearing or waiting for in-flight PCM.
        // The writer retains its original tag and its final check will fail;
        // the next bounded rearm attempt may advance after that claim drains.
        buffer.epoch.fetch_or(REQUESTED, Ordering::AcqRel);
        return 0;
    };
    let previous = buffer.epoch.load(Ordering::Acquire);
    let generation = previous & MAX_EPOCH;
    if generation == 0 || previous & !(REQUESTED | MAX_EPOCH) != 0 {
        return 0;
    }
    if generation == MAX_EPOCH {
        // Exhaustion is visible to control even when this invalidation was not
        // preceded by a reader request. Never reuse a remote reader's generation.
        let _ = buffer.epoch.compare_exchange(
            previous,
            previous | REQUESTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        return 0;
    }
    let next = generation + 1;
    buffer
        .epoch
        .compare_exchange(previous, next, Ordering::AcqRel, Ordering::Acquire)
        .map_or(0, |_| next)
}

/// Publishes original Float32 bits only into the caller's observed epoch. The
/// producer claim is acquired before checking it, so a control rearm between
/// snapshot observation and this call cannot retag old-format PCM as new audio.
/// # Safety
/// Mapping geometry must be valid and immutable. Exactly one PCM writer owns
/// this call; the non-null source must contain `frame_count * channels` samples
/// disjoint from the shared mapping and remain readable throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_write(
    buffer: *mut SharedAudioBuffer,
    expected_epoch: u64,
    start_frame: u64,
    samples: *const f32,
    frame_count: u32,
) -> bool {
    let Some(buffer) = (unsafe { buffer.as_ref() }) else {
        return false;
    };
    let Some(end) = range_end(start_frame, frame_count) else {
        return false;
    };
    if frame_count != 0 && samples.is_null() {
        return false;
    }
    let Some(_producer) = buffer.claim_producer() else {
        return false;
    };
    if !active(expected_epoch) || buffer.epoch.load(Ordering::Acquire) != expected_epoch {
        return false;
    }
    let epoch = expected_epoch;
    if frame_count == 0 {
        return true;
    }
    let previous = buffer.published_through_frame.load(Ordering::Acquire);
    if previous >> FRAME_BITS == epoch && start_frame < (previous & FRAME_MASK) {
        return false;
    }
    let channels = buffer.channels as usize;
    for frame in 0..frame_count as usize {
        let absolute = start_frame + frame as u64;
        let Ok(slot) = usize::try_from(absolute & u64::from(buffer.capacity_mask)) else {
            return false;
        };
        let cell = unsafe { buffer.stamp_cell(slot) };
        cell.store(0, Ordering::Relaxed);
        // A reader seeing any replacement sample synchronizes these fences,
        // forcing its final stamp read to observe invalidation or the new tag.
        fence(Ordering::Release);
        for channel in 0..channels {
            let bits = unsafe { samples.add(frame * channels + channel).read() }.to_bits();
            unsafe { buffer.sample_cell(slot * channels + channel) }.store(bits, Ordering::Relaxed);
        }
        cell.store(stamp(epoch, absolute), Ordering::Release);
    }
    // A concurrent rearm cannot publish an old absolute end as a new timeline:
    // the watermark carries the epoch originally observed by this writer.
    buffer
        .published_through_frame
        .store(stamp(epoch, end), Ordering::Release);
    if buffer.epoch.load(Ordering::Acquire) != epoch {
        return false;
    }
    buffer
        .written_frames
        .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    true
}

/// Always fills a valid request; unavailable or invalidated frames are silence.
/// # Safety
/// Mapping geometry must be valid and immutable. The non-null destination must
/// hold `frame_count * channels` writable samples disjoint from the mapping and
/// from other concurrent output buffers. Invalid pointer/range requests do no I/O.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_read(
    buffer: *mut SharedAudioBuffer,
    start_frame: u64,
    samples: *mut f32,
    frame_count: u32,
) -> u32 {
    let Some(buffer) = (unsafe { buffer.as_ref() }) else {
        return 0;
    };
    if range_end(start_frame, frame_count).is_none() || (frame_count != 0 && samples.is_null()) {
        return 0;
    }
    // SAFETY: the ABI caller supplies the complete exclusive destination.
    unsafe { read_frames(buffer, start_frame, samples, frame_count, None) }
}

/// Reads PCM and marks each committed frame, including valid digital silence.
/// Invalid ranges or mismatched slice lengths return silence and no coverage.
///
/// # Safety
/// A non-null buffer must reference a live initialized mapping with immutable
/// geometry. The output slices must be disjoint from that mapping.
pub unsafe fn read_with_frame_coverage(
    buffer: *mut SharedAudioBuffer,
    start_frame: u64,
    samples: &mut [f32],
    coverage: &mut [bool],
) -> u32 {
    coverage.fill(false);
    let Some(buffer) = (unsafe { buffer.as_ref() }) else {
        samples.fill(0.0);
        return 0;
    };
    let Ok(frame_count) = u32::try_from(coverage.len()) else {
        samples.fill(0.0);
        return 0;
    };
    if coverage.len().checked_mul(buffer.channels as usize) != Some(samples.len())
        || range_end(start_frame, frame_count).is_none()
    {
        samples.fill(0.0);
        return 0;
    }
    // SAFETY: checked geometry covers both exclusive slices; the caller retains the mapping.
    unsafe {
        read_frames(
            buffer,
            start_frame,
            samples.as_mut_ptr(),
            frame_count,
            Some(coverage),
        )
    }
}

// SAFETY: samples holds frame_count * channels writable values, and optional
// coverage holds frame_count writable flags, all disjoint from the live mapping.
unsafe fn read_frames(
    buffer: &SharedAudioBuffer,
    start_frame: u64,
    samples: *mut f32,
    frame_count: u32,
    mut coverage: Option<&mut [bool]>,
) -> u32 {
    let channels = buffer.channels as usize;
    let epoch = buffer.epoch.load(Ordering::Acquire);
    let mut available = 0;
    if active(epoch) {
        for frame in 0..frame_count as usize {
            let absolute = start_frame + frame as u64;
            let Ok(slot) = usize::try_from(absolute & u64::from(buffer.capacity_mask)) else {
                return 0;
            };
            let expected = stamp(epoch, absolute);
            let cell = unsafe { buffer.stamp_cell(slot) };
            if cell.load(Ordering::Acquire) == expected {
                for channel in 0..channels {
                    let bits = unsafe { buffer.sample_cell(slot * channels + channel) }
                        .load(Ordering::Relaxed);
                    unsafe {
                        samples
                            .add(frame * channels + channel)
                            .write(f32::from_bits(bits));
                    }
                }
                fence(Ordering::Acquire);
                if cell.load(Ordering::Acquire) == expected {
                    available += 1;
                    if let Some(coverage) = &mut coverage {
                        coverage[frame] = true;
                    }
                    continue;
                }
            }
            unsafe {
                ptr::write_bytes(samples.add(frame * channels), 0, channels);
            }
        }
    }
    // A rearm arriving during the copy must not return previous-epoch samples.
    if !active(epoch) || buffer.epoch.load(Ordering::Acquire) != epoch {
        if frame_count != 0 {
            unsafe {
                ptr::write_bytes(samples, 0, frame_count as usize * channels);
            }
        }
        available = 0;
        if let Some(coverage) = &mut coverage {
            coverage.fill(false);
        }
    }
    buffer
        .read_frames
        .fetch_add(u64::from(frame_count), Ordering::Relaxed);
    buffer
        .missing_frames
        .fetch_add(u64::from(frame_count - available), Ordering::Relaxed);
    available
}

/// Counters are observational; epoch and its watermark are sampled coherently.
/// # Safety
/// Non-null pointers must reference an initialized, attached, live mapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_shared_audio_buffer_stats(
    buffer: *const SharedAudioBuffer,
) -> SharedAudioBufferStats {
    let Some(buffer) = (unsafe { buffer.as_ref() }) else {
        return SharedAudioBufferStats::default();
    };
    let (epoch, published_through_frame) = buffer.publication();
    SharedAudioBufferStats {
        written_frames: buffer.written_frames.load(Ordering::Relaxed),
        read_frames: buffer.read_frames.load(Ordering::Relaxed),
        missing_frames: buffer.missing_frames.load(Ordering::Relaxed),
        epoch,
        published_through_frame,
    }
}

#[cfg(test)]
#[path = "unit_tests/shared_audio_buffer.rs"]
mod tests;
