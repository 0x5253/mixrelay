//! Epoch-tagged source coordinates in the same shared mapping as application PCM.
use super::ApplicationClock;
use std::{
    ptr,
    sync::atomic::{AtomicU64, Ordering, fence},
};

// SAFETY: clock is a live mapping with required_bytes(capacity, channels) storage.
unsafe fn cell(clock: *const ApplicationClock, capacity: u32, frame: u64) -> *mut AtomicU64 {
    let slot = usize::try_from(frame & u64::from(capacity - 1)).unwrap_or(0);
    // SAFETY: immutable validated capacity; two atomic u64 cells per frame after the header.
    unsafe {
        clock
            .cast::<u8>()
            .add(size_of::<ApplicationClock>() + slot * 16)
            .cast_mut()
            .cast()
    }
}

// SAFETY: unpublished mapping exclusively owns the complete initialized sidecar extent.
pub(super) unsafe fn initialize(clock: *mut ApplicationClock, capacity: u32) {
    for frame in 0..capacity {
        // SAFETY: every slot is visited once before publication.
        unsafe {
            cell(clock, capacity, u64::from(frame)).write(AtomicU64::new(0));
            cell(clock, capacity, u64::from(frame))
                .add(1)
                .write(AtomicU64::new(0));
        }
    }
}

impl ApplicationClock {
    /// # Safety
    /// Retained mapping with immutable capacity; caller owns the producer guard and epoch.
    pub(crate) unsafe fn write_times(
        &self,
        capacity: u32,
        epoch: u64,
        start: u64,
        times: impl Iterator<Item = u64>,
    ) {
        for (offset, nanos) in times.enumerate() {
            let frame = start + offset as u64;
            // SAFETY: validated shared sidecar; access is atomic in every process.
            let cells = unsafe { cell(ptr::from_ref(self), capacity, frame) };
            // SAFETY: both cells are initialized atomic u64s in this mapping;
            // producer ownership excludes another writer to this coordinate.
            unsafe {
                (*cells).store(0, Ordering::Relaxed);
                fence(Ordering::Release);
                (*cells.add(1)).store(nanos, Ordering::Relaxed);
                (*cells).store((epoch << 48) | frame, Ordering::Release);
            }
        }
    }

    /// # Safety
    /// Retained mapping with immutable capacity; caller verifies the PCM epoch after its read.
    pub(crate) unsafe fn time_at(&self, capacity: u32, epoch: u64, frame: u64) -> Option<u64> {
        // SAFETY: frame is masked into the validated shared sidecar; all accesses are atomic.
        let cells = unsafe { cell(ptr::from_ref(self), capacity, frame) };
        let expected = (epoch << 48) | frame;
        // SAFETY: both initialized cells remain mapped; atomic tag checks reject
        // concurrent replacement without following any shared-memory pointers.
        unsafe {
            if (*cells).load(Ordering::Acquire) != expected {
                return None;
            }
            let nanos = (*cells.add(1)).load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            ((*cells).load(Ordering::Acquire) == expected && nanos != 0).then_some(nanos)
        }
    }
}
