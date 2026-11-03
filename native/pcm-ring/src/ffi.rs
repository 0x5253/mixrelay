use super::{PcmRing, RingStats, SampleFormat};
use std::{
    ffi::c_void,
    ptr, slice,
    sync::atomic::Ordering::{Acquire, Release},
};

/// Publishes a whole block of neutral PCM without external input storage.
/// Uses the same capacity, epoch acknowledgement and release publication as PCM writes.
/// # Safety
/// Retained initialized ring with exactly one serialized producer. The consumer
/// exclusively owns its reader cursor and cannot access these unpublished spans.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_write_silence(ring: *mut PcmRing, frames: u32) -> u32 {
    // SAFETY: retained native owner; immutable geometry is shared with the consumer.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if ring.storage.samples.is_null() {
        return 0;
    }
    // SAFETY: only the serialized producer accesses its epoch cell.
    let writer = unsafe { &mut *ring.writer_epoch.get() };
    let Some(cursor) = ring.reserve_write(writer, frames) else {
        return 0;
    };
    let (start, first, second) = ring.spans(cursor, frames);
    // SAFETY: the acquired consumer cursor grants exclusive access to these
    // free spans. All-zero IEEE 754 bytes represent positive f32 silence.
    unsafe {
        ptr::write_bytes(ring.storage.samples.add(start), 0, first);
        if second != 0 {
            ptr::write_bytes(ring.storage.samples, 0, second);
        }
    }
    ring.publication
        .written
        .store(cursor.wrapping_add(u64::from(frames)), Release);
    frames
}

/// Writes interleaved LE PCM directly into free float ring spans. Invalid format
/// IDs/null input return zero; valid zero frames still acknowledge the epoch.
/// # Safety
/// Retained initialized ring and exactly one serialized producer. For nonzero
/// frames input is readable for `frames * channels * format.bytes_per_sample()` bytes,
/// disjoint from the ring mapping, and retained until return. No byte alignment
/// is required. No consumer or other writer may access the reserved free spans.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_write_pcm(
    ring: *mut PcmRing,
    input: *const c_void,
    frames: u32,
    format: u32,
) -> u32 {
    let Some(format) = SampleFormat::from_raw(format) else {
        return 0;
    };
    if format == SampleFormat::Float32
        && cfg!(target_endian = "little")
        && input.addr().is_multiple_of(align_of::<f32>())
    {
        // SAFETY: byte extent equals the float extent and alignment is checked.
        return unsafe { mr_pcm_ring_write(ring, input.cast(), frames) };
    }
    // SAFETY: retained native owner; immutable geometry is shared with the consumer.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if ring.storage.samples.is_null() || (frames != 0 && input.is_null()) {
        return 0;
    }
    // SAFETY: only the serialized producer accesses its epoch cell.
    let writer = unsafe { &mut *ring.writer_epoch.get() };
    let Some(cursor) = ring.reserve_write(writer, frames) else {
        return 0;
    };
    let (start, first, second) = ring.spans(cursor, frames);
    let width = format.bytes_per_sample();
    // SAFETY: caller's complete byte extent is disjoint from the mapping. The
    // consumer's acquire/release cursor grants precisely these free spans, not
    // the whole allocation. Their lengths sum to frames*channels samples.
    unsafe {
        let bytes = slice::from_raw_parts(input.cast::<u8>(), (first + second) * width);
        let destination = slice::from_raw_parts_mut(ring.storage.samples.add(start), first);
        let _ = format.decode(&bytes[..first * width], destination);
        if second != 0 {
            let destination = slice::from_raw_parts_mut(ring.storage.samples, second);
            let _ = format.decode(&bytes[first * width..], destination);
        }
    }
    ring.publication
        .written
        .store(cursor.wrapping_add(u64::from(frames)), Release);
    frames
}

/// Reads interleaved LE PCM after the existing two-quantum prefill, silencing
/// the complete byte output on shortage/epoch invalidation. Invalid format IDs
/// return zero without touching output, since their byte width is unknown.
/// # Safety
/// Retained initialized ring and exactly one serialized consumer (including
/// discard). For nonzero frames output is writable for
/// `frames * channels * format.bytes_per_sample()` bytes, disjoint from the mapping and
/// all other live references. The buffer needs no float alignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_read_pcm(
    ring: *mut PcmRing,
    output: *mut c_void,
    frames: u32,
    format: u32,
) -> u32 {
    let Some(format) = SampleFormat::from_raw(format) else {
        return 0;
    };
    if format == SampleFormat::Float32
        && cfg!(target_endian = "little")
        && output.addr().is_multiple_of(align_of::<f32>())
    {
        // SAFETY: byte extent equals float extent and alignment is checked.
        return unsafe { mr_pcm_ring_read(ring, output.cast(), frames) };
    }
    // SAFETY: initialized owner retained throughout the serialized consumer call.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if ring.storage.samples.is_null() || output.is_null() || frames == 0 {
        return 0;
    }
    let count = frames as usize * ring.channels as usize * format.bytes_per_sample();
    // SAFETY: exclusive caller buffer, sized as documented, independent of ring spans.
    let bytes = unsafe { slice::from_raw_parts_mut(output.cast::<u8>(), count) };
    // SAFETY: only this consumer touches its private reader cell.
    let reader = unsafe { &mut *ring.reader.get() };
    if let Some(window) = ring.reserve_read(reader, frames, true) {
        let (start, first, second) = ring.spans(window.cursor, frames);
        let split = first * format.bytes_per_sample();
        // SAFETY: acquired producer publication grants immutable access to only
        // these completed spans until finish_read releases them for producer reuse.
        unsafe {
            let source = slice::from_raw_parts(ring.storage.samples.add(start), first);
            let _ = format.encode(source, &mut bytes[..split]);
            if second != 0 {
                let source = slice::from_raw_parts(ring.storage.samples, second);
                let _ = format.encode(source, &mut bytes[split..]);
            }
        }
        if ring.finish_read(reader, &window, frames) {
            return frames;
        }
    }
    bytes.fill(0);
    0
}

/// Initializes exclusive native storage; invalid geometry/mapping failure returns
/// false with an empty, clearable ring. Call only on fresh or cleared storage.
/// # Safety
/// Non-null ring is 120 writable bytes aligned to 8. The native stream owns it
/// exclusively before IO registration; it contains no live mapping to overwrite.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_init(
    ring: *mut PcmRing,
    channels: u32,
    capacity: u32,
) -> bool {
    if ring.is_null() {
        return false;
    }
    let prepared = PcmRing::new(channels, capacity);
    let success = prepared.is_some();
    // SAFETY: fresh/cleared sized storage; failed setup is also safe to clear.
    unsafe {
        ring.write(prepared.unwrap_or_default());
    }
    success
}

/// Unmaps PCM and returns storage to the empty state. Null/zero storage is harmless.
/// # Safety
/// Native stream owns initialized or zeroed storage. Producer, consumer and stat
/// readers must have stopped; no Rust borrow may survive this control-side clear.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_clear(ring: *mut PcmRing) {
    // SAFETY: native zero initialization is a valid empty value for every field.
    if let Some(ring) = unsafe { ring.as_mut() } {
        *ring = PcmRing::default();
    }
}

/// Invalidates the current epoch without waiting for either realtime owner.
/// # Safety
/// Ring remains initialized/live under native stream ownership throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_restart(ring: *mut PcmRing) {
    // SAFETY: retained owner; only the atomic control request is mutated.
    if let Some(ring) = unsafe { ring.as_ref() } {
        ring.publication.requested_epoch.fetch_add(1, Release);
    }
}

/// Consumer-only discard of published PCM; producer cursor is never changed.
/// # Safety
/// The native consumer exclusively owns its private reader cell, with the ring
/// retained. No other consumer/read/discard may overlap; producer may continue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_discard(ring: *mut PcmRing) {
    // SAFETY: initialized ring retained by the native stream.
    if let Some(ring) = unsafe { ring.as_ref() } {
        ring.publication
            .read
            .store(ring.publication.written.load(Acquire), Release);
        // SAFETY: only this serialized consumer touches its reader cell.
        unsafe {
            (*ring.reader.get()).primed = false;
        }
    }
}

/// Writes a whole block, retaining unread PCM on overflow. Zero frames still ACK
/// a pending epoch. Nonzero input must be non-null; null input returns zero.
/// # Safety
/// Retained initialized ring, one serialized producer. Accepted input contains
/// frames*channels readable floats disjoint from the mapped ring allocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_write(
    ring: *mut PcmRing,
    input: *const f32,
    frames: u32,
) -> u32 {
    // SAFETY: same retained owner, sole producer and complete input extent.
    unsafe { write_observed(ring, input, frames, |_| {}) }
}

pub(super) unsafe fn write_observed(
    ring: *mut PcmRing,
    input: *const f32,
    frames: u32,
    before_publish: impl FnOnce(u64),
) -> u32 {
    // SAFETY: native stream retains storage; writer mutation is inside its cell.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if ring.storage.samples.is_null() || (frames != 0 && input.is_null()) {
        return 0;
    }
    // SAFETY: this native callback/worker is the sole producer for this ring.
    let writer = unsafe { &mut *ring.writer_epoch.get() };
    let Some(cursor) = ring.reserve_write(writer, frames) else {
        return 0;
    };
    let (start, first, second) = ring.spans(cursor, frames);
    // SAFETY: acquire of read grants only these free, bounded mapping spans;
    // no mutable reference to the whole mapping overlaps the consumer.
    unsafe {
        ptr::copy_nonoverlapping(input, ring.storage.samples.add(start), first);
        if second != 0 {
            ptr::copy_nonoverlapping(input.add(first), ring.storage.samples, second);
        }
    }
    before_publish(cursor);
    ring.publication
        .written
        .store(cursor.wrapping_add(u64::from(frames)), Release);
    frames
}

// SAFETY: retained initialized ring and serialized consumer; output is writable
// for frames*channels floats, disjoint from the mapping, including rejected reads.
pub(super) unsafe fn read(
    ring: *mut PcmRing,
    output: *mut f32,
    frames: u32,
    prefill: bool,
    after_copy: impl FnOnce(&PcmRing),
) -> u32 {
    // SAFETY: native stream retains storage while its consumer is running.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return 0;
    };
    if ring.storage.samples.is_null() || output.is_null() || frames == 0 {
        return 0;
    }
    let count = frames as usize * ring.channels as usize;
    // SAFETY: only this serialized consumer touches reader state.
    let reader = unsafe { &mut *ring.reader.get() };
    if let Some(window) = ring.reserve_read(reader, frames, prefill) {
        let (start, first, second) = ring.spans(window.cursor, frames);
        // SAFETY: acquire observed completed producer PCM. Only these published
        // spans are read, before the consumer release permits their reuse.
        unsafe {
            ptr::copy_nonoverlapping(ring.storage.samples.add(start), output, first);
            if second != 0 {
                ptr::copy_nonoverlapping(ring.storage.samples, output.add(first), second);
            }
        }
        after_copy(ring);
        if ring.finish_read(reader, &window, frames) {
            return frames;
        }
    }
    // SAFETY: rejected/invalidated reads still own the complete caller output.
    unsafe {
        ptr::write_bytes(output, 0, count);
    }
    0
}

/// Reads a whole block after two-quantum prefill, otherwise silences the full output.
/// # Safety
/// Native stream retains ring and grants this call sole consumer ownership. Output
/// is writable for frames*channels floats, disjoint from PCM storage, even on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_read(
    ring: *mut PcmRing,
    output: *mut f32,
    frames: u32,
) -> u32 {
    // SAFETY: native consumer owns reader state and the complete output block.
    unsafe { read(ring, output, frames, true, |_| {}) }
}

/// Reads a whole block without prefill; retains epoch checks and failure silence.
/// # Safety
/// Same retained ring, serialized consumer and complete output contract as read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_read_available(
    ring: *mut PcmRing,
    output: *mut f32,
    frames: u32,
) -> u32 {
    // SAFETY: native consumer owns reader state and the complete output block.
    unsafe { read(ring, output, frames, false, |_| {}) }
}

/// Returns independent diagnostic observations; null returns an empty record.
/// # Safety
/// Native stream retains initialized or zeroed ring storage until this read ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_pcm_ring_stats(ring: *const PcmRing) -> RingStats {
    // SAFETY: only immutable geometry and atomic observations are accessed.
    unsafe { ring.as_ref() }.map_or_else(RingStats::default, PcmRing::stats)
}
