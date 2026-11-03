//! Callback SPSC device PCM, independent of `CoreAudio` and `PipeWire` policy.
//! Supports x86-64/AArch64 macOS/Linux native consumers.
#![cfg(any(target_os = "macos", target_os = "linux"))]

// The embedded C layout and anonymous-mapping ABI are defined for these targets.
// Do not silently use their constants on a different Unix ABI.
#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    target_pointer_width = "64"
)))]
compile_error!("native PCM storage supports only 64-bit x86-64/AArch64 macOS/Linux");

use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};
mod ffi;
mod queue;
mod sample_format;
mod storage;
pub use ffi::{
    mr_pcm_ring_clear, mr_pcm_ring_discard, mr_pcm_ring_init, mr_pcm_ring_read,
    mr_pcm_ring_read_available, mr_pcm_ring_read_pcm, mr_pcm_ring_restart, mr_pcm_ring_stats,
    mr_pcm_ring_write, mr_pcm_ring_write_pcm, mr_pcm_ring_write_silence,
};
pub use sample_format::SampleFormat;
pub use storage::prefault_pcm;

/// Acknowledges a published producer epoch before observing retained PCM metadata.
/// Returns `None` while the producer has not acknowledged a restart. The snapshot
/// protects no copied PCM; a subsequent read still validates its own reservation.
/// # Safety
/// Retained initialized ring; caller is its sole serialized consumer, including reads/discards.
pub unsafe fn consumer_stats(ring: *mut PcmRing) -> Option<RingStats> {
    // SAFETY: caller retains the ring and exclusively owns its private reader state.
    let ring = unsafe { ring.as_ref() }?;
    let reader = unsafe { &mut *ring.reader.get() };
    let previous = (reader.epoch, reader.primed);
    let window = ring.reserve_read(reader, 0, false)?;
    // Observing metadata cannot satisfy a future PCM read's prefill requirement.
    reader.primed = previous.0 == window.epoch && previous.1;
    let stats = ring.stats();
    (window.epoch == ring.publication.requested_epoch.load(Ordering::Acquire)).then_some(stats)
}

/// Advances the sole consumer over stale frames without copying them.
/// # Safety
/// Retained initialized ring; caller is the serialized consumer, including discard/read.
pub unsafe fn skip_available(ring: *mut PcmRing, frames: u32) -> bool {
    // SAFETY: only this consumer accesses its private reader state.
    let Some(ring) = (unsafe { ring.as_ref() }) else {
        return false;
    };
    let reader = unsafe { &mut *ring.reader.get() };
    ring.reserve_read(reader, frames, false)
        .is_some_and(|window| ring.finish_read(reader, &window, frames))
}

/// Copies a whole available block and observes its absolute cursor before releasing it.
/// # Safety
/// Same ring/sole-consumer/output extent contract as `read_available`. The observer may
/// read a matching metadata sidecar but cannot mutate or recursively consume the PCM ring.
pub unsafe fn read_available_observed(
    ring: *mut PcmRing,
    output: *mut f32,
    frames: u32,
    observe: impl FnOnce(u64),
) -> u32 {
    // SAFETY: the observer runs synchronously while reserve_read still protects these slots.
    unsafe {
        ffi::read(ring, output, frames, false, |ring| {
            observe(ring.stats().read);
        })
    }
}

/// Writes PCM and publishes its metadata only after reserving free ring slots.
/// # Safety
/// Same sole-producer, retained ring and complete immutable input contract as write.
/// The observer must not access PCM or change either ring cursor.
pub unsafe fn write_observed(
    ring: *mut PcmRing,
    input: *const f32,
    frames: u32,
    observe: impl FnOnce(u64),
) -> u32 {
    // SAFETY: the observer runs after PCM copy and before the release publication.
    unsafe { ffi::write_observed(ring, input, frames, observe) }
}

/// Embedded opaque native storage. Zero-initialization owns no resources; init
/// must finish before callbacks start. Never copy, clear or reinitialize it live.
/// Producer and consumer have distinct private cells; observers use atomics only.
#[repr(C)]
#[derive(Debug, Default)]
pub struct PcmRing {
    storage: storage::Storage,
    channels: u32,
    capacity: u32,
    publication: Publication,
    writer_epoch: UnsafeCell<u64>,
    reader: UnsafeCell<Reader>,
}

#[repr(C)]
#[derive(Debug, Default)]
struct Publication {
    written: AtomicU64,
    read: AtomicU64,
    quantum: AtomicU32,
    overflows: AtomicU64,
    starvations: AtomicU64,
    requested_epoch: AtomicU64,
    epoch_sequence: AtomicU64,
    published_epoch: AtomicU64,
    epoch_start: AtomicU64,
}

#[repr(C)]
#[derive(Debug, Default)]
struct Reader {
    epoch: u64,
    primed: bool,
}

/// Diagnostic loads are independent observations, not a transactional snapshot.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct RingStats {
    /// Consumer's released frame position.
    pub read: u64,
    /// Producer's published frame position.
    pub written: u64,
    /// Number of capacity-rejected write calls, not dropped frames.
    pub overflows: u64,
    /// Number of primed-to-starved transitions.
    pub starvations: u64,
    /// Immutable initialized channel width.
    pub channels: u32,
    /// Available frames, or zero for an incoherent/out-of-range observation.
    pub queued: u32,
}

const _: () = assert!(size_of::<PcmRing>() == 120 && align_of::<PcmRing>() == 8);
const _: () = assert!(size_of::<RingStats>() == 40 && align_of::<RingStats>() == 8);
const _: () = assert!(cfg!(target_has_atomic = "64") && cfg!(target_has_atomic = "32"));

impl PcmRing {
    /// Allocates and prefaults a device-owned ring before producer/consumer publication.
    /// Returns `None` for unsupported geometry or mapping failure. Dropping the
    /// owner releases its mapping and requires all ring users to have drained.
    #[must_use]
    pub fn new(channels: u32, capacity: u32) -> Option<Self> {
        if !(1..=64).contains(&channels) || !(4..=1 << 20).contains(&capacity) {
            return None;
        }
        Some(Self {
            storage: storage::Storage::new(channels as usize * capacity as usize).ok()?,
            channels,
            capacity,
            ..Self::default()
        })
    }

    /// Reads immutable geometry and independent atomic diagnostic counters.
    /// An inconsistent cursor pair beyond the capacity reports an empty queue.
    #[must_use]
    pub fn stats(&self) -> RingStats {
        let written = self.publication.written.load(Ordering::Acquire);
        let read = self.publication.read.load(Ordering::Acquire);
        let available = written.wrapping_sub(read);
        RingStats {
            read,
            written,
            overflows: self.publication.overflows.load(Ordering::Relaxed),
            starvations: self.publication.starvations.load(Ordering::Relaxed),
            channels: self.channels,
            queued: u32::try_from(available)
                .ok()
                .filter(|queued| *queued <= self.capacity)
                .unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests;
