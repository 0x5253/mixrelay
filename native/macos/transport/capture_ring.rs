//! App-capture SPSC storage: whole-block writes and immediate partial reads.

use std::{
    alloc::Layout,
    cell::UnsafeCell,
    ptr::{self, NonNull},
    sync::atomic::{AtomicU64, Ordering},
};

mod ffi;
pub use ffi::{
    mr_audio_ring_available_frames, mr_audio_ring_capacity_frames, mr_audio_ring_create,
    mr_audio_ring_destroy, mr_audio_ring_dropped_frames, mr_audio_ring_read_interleaved,
    mr_audio_ring_read_timed, mr_audio_ring_write_interleaved, mr_audio_ring_write_planar,
    write_timed_interleaved, write_timed_planar,
};

/// Opaque native owner, created before the tap `IOProc` and destroyed after its
/// callback, pump consumer and stat readers drain. Never copy a live ring.
/// The cells permit only cursor-owned, disjoint PCM ranges to be accessed; this
/// type deliberately does not implement `Sync` or expose safe concurrent access.
#[derive(Debug)]
pub struct AudioRing {
    samples: Vec<UnsafeCell<f32>>,
    capacity: usize,
    channels: usize,
    read: AtomicU64,
    written: AtomicU64,
    dropped: AtomicU64,
    times: crate::frame_times::FrameTimes,
}

fn allocate_samples(count: usize) -> Option<Vec<UnsafeCell<f32>>> {
    let mut samples = Vec::new();
    samples.try_reserve_exact(count).ok()?;
    samples.resize_with(count, || UnsafeCell::new(0.0));
    Some(samples)
}

// Separate fallible allocation boundaries allow deterministic failure injection
// without exposing allocator callbacks in the native/runtime contract.
// SAFETY: allocate_owner returns None or fresh global-allocator storage for Layout.
unsafe fn create(
    capacity: usize,
    channels: u32,
    samples: impl FnOnce(usize) -> Option<Vec<UnsafeCell<f32>>>,
    allocate_owner: impl FnOnce(Layout) -> Option<NonNull<u8>>,
) -> *mut AudioRing {
    if !capacity.is_power_of_two() || capacity > 1 << 20 || !(1..=64).contains(&channels) {
        return ptr::null_mut();
    }
    let Ok(channels) = usize::try_from(channels) else {
        return ptr::null_mut();
    };
    let Some(samples) = samples(capacity * channels) else {
        return ptr::null_mut();
    };
    let Some(times) = crate::frame_times::FrameTimes::new(capacity) else {
        return ptr::null_mut();
    };
    let Some(owner) = allocate_owner(Layout::new::<AudioRing>()) else {
        // The prepared PCM vector is dropped before a failed owner allocation returns.
        return ptr::null_mut();
    };
    let owner = owner.cast::<AudioRing>();
    // SAFETY: the fresh, sized/aligned allocation has no published readers yet.
    unsafe {
        owner.as_ptr().write(AudioRing {
            samples,
            capacity,
            channels,
            read: AtomicU64::new(0),
            written: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            times,
        });
    }
    owner.as_ptr()
}

impl AudioRing {
    fn reserve(&self, frames: usize) -> Option<u64> {
        if frames == 0 {
            return None;
        }
        let written = self.written.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        let occupied = written.wrapping_sub(read);
        if frames > self.capacity || occupied > (self.capacity - frames) as u64 {
            self.drop_frames(frames);
            return None;
        }
        Some(written)
    }

    fn drop_frames(&self, frames: usize) {
        self.dropped.fetch_add(frames as u64, Ordering::Relaxed);
    }

    fn publish(&self, written: u64, frames: usize) {
        self.written
            .store(written.wrapping_add(frames as u64), Ordering::Release);
    }

    fn read_window(&self, maximum: usize) -> (u64, usize) {
        let read = self.read.load(Ordering::Relaxed);
        let written = self.written.load(Ordering::Acquire);
        let limit = maximum.min(self.capacity);
        // An occupancy beyond usize necessarily exceeds the bounded window.
        let frames = usize::try_from(written.wrapping_sub(read)).map_or(limit, |f| f.min(limit));
        (read, frames)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "power-of-two capacity within usize: the mask keeps only low cursor bits"
    )]
    fn split(&self, cursor: u64, frames: usize) -> (usize, usize) {
        let start = cursor as usize & (self.capacity - 1);
        (
            start * self.channels,
            frames.min(self.capacity - start) * self.channels,
        )
    }
}

#[cfg(test)]
mod tests;
