//! Single-callback continuity and bounded atomic observations for native readers.
//! This proves one stream's timeline, never synchronization between devices.

use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst},
};

/// Native caller-owned storage. Initialize before publication; never copy or clear
/// a live clock. Only the serialized `IOProc` records; reset requires it to drain.
/// Readers access only the atomic publication, not the callback-owned writer cell.
#[repr(C)]
#[derive(Debug, Default)]
pub struct CallbackClock {
    writer: UnsafeCell<Continuity>,
    publication: Publication,
}

#[repr(C)]
#[derive(Debug, Default)]
struct Continuity {
    primed: bool,
    next_sample: f64,
    previous_host: u64,
}

#[repr(C)]
#[derive(Debug, Default)]
struct Publication {
    sequence: AtomicU64,
    sample_bits: AtomicU64,
    host_time: AtomicU64,
    discontinuities: AtomicU64,
    valid: AtomicU32,
}

/// Coherent native snapshot; invalid timestamps retain accumulated discontinuities.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CallbackClockSnapshot {
    /// Native buffer position, not callback arrival time.
    pub sample_time: f64,
    /// Native buffer host-clock timestamp.
    pub host_time: u64,
    /// Cumulative timeline breaks, wrapping only at the integer limit.
    pub discontinuities: u64,
    /// One for a valid timestamp, zero otherwise.
    pub valid: u32,
}

const _: () = assert!(size_of::<CallbackClock>() == 64);
const _: () = assert!(align_of::<CallbackClock>() == 8);
const _: () = assert!(size_of::<CallbackClockSnapshot>() == 32);
const _: () = assert!(align_of::<CallbackClockSnapshot>() == 8);
const _: () = assert!(cfg!(target_has_atomic = "64") && cfg!(target_has_atomic = "32"));

impl Continuity {
    fn record(
        &mut self,
        sample: f64,
        host: u64,
        timestamp_valid: bool,
        frames: u64,
    ) -> (bool, bool) {
        let valid = timestamp_valid
            && frames != 0
            && sample.is_finite()
            && sample.abs() < 4_503_599_627_370_496.0;
        let broken = self.primed
            && (!valid || (sample - self.next_sample).abs() > 0.5 || host <= self.previous_host);
        self.primed = valid;
        if valid {
            // Explicit word conversion retains f64 rounding. Native callbacks
            // use bounded u32 frames, but the ABI accepts a full u64 frame count.
            let [l0, l1, l2, l3, h0, h1, h2, h3] = frames.to_le_bytes();
            let high = u32::from_le_bytes([h0, h1, h2, h3]);
            let low = u32::from_le_bytes([l0, l1, l2, l3]);
            self.next_sample = sample + f64::from(high).mul_add(4_294_967_296.0, f64::from(low));
            self.previous_host = host;
        }
        (valid, broken)
    }
}

impl Publication {
    fn publish(&self, sample: f64, host: u64, valid: bool, broken: bool) {
        // SC preserves the former C ordering on arm64/x86_64. Exactly one writer;
        // fetch_add wraps and never panics at counter rollover.
        self.sequence.fetch_add(1, SeqCst);
        self.sample_bits
            .store(if valid { sample.to_bits() } else { 0 }, SeqCst);
        self.host_time.store(if valid { host } else { 0 }, SeqCst);
        if broken {
            self.discontinuities.fetch_add(1, SeqCst);
        }
        self.valid.store(u32::from(valid), SeqCst);
        self.sequence.fetch_add(1, SeqCst);
    }

    fn snapshot(&self) -> Option<CallbackClockSnapshot> {
        let before = self.sequence.load(SeqCst);
        if before & 1 != 0 {
            return None;
        }
        let snapshot = CallbackClockSnapshot {
            sample_time: f64::from_bits(self.sample_bits.load(SeqCst)),
            host_time: self.host_time.load(SeqCst),
            discontinuities: self.discontinuities.load(SeqCst),
            valid: self.valid.load(SeqCst),
        };
        (before == self.sequence.load(SeqCst)).then_some(snapshot)
    }
}

/// Initializes exclusively owned native storage. Null is a no-op.
/// # Safety
/// Non-null `clock` is 64 writable bytes aligned to 8, with no concurrent users.
/// The embedding MRDeviceIO/MRProcessTap owner initializes before registering IO.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_callback_clock_init(clock: *mut CallbackClock) {
    if !clock.is_null() {
        // SAFETY: exclusive caller-owned sized/aligned storage, without a live borrow.
        unsafe { clock.write(CallbackClock::default()) };
    }
}

/// Records projected native timestamp fields without allocation, locks or retries.
/// # Safety
/// Non-null `clock` is initialized and retained by its registered `IOProc` owner.
/// Only that serialized callback records; reset cannot overlap it. Snapshot/counter
/// readers may run concurrently while the native owner keeps this storage live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_callback_clock_record_values(
    clock: *mut CallbackClock,
    sample: f64,
    host: u64,
    timestamp_valid: bool,
    frames: u64,
) {
    // SAFETY: owner retains initialized storage; writer mutation is within UnsafeCell.
    let Some(clock) = (unsafe { clock.as_ref() }) else {
        return;
    };
    // SAFETY: only the serialized IOProc touches writer state; readers touch Publication.
    let writer = unsafe { &mut *clock.writer.get() };
    let (valid, broken) = writer.record(sample, host, timestamp_valid, frames);
    clock.publication.publish(sample, host, valid, broken);
}

/// Starts a new callback timeline without clearing accumulated diagnostics.
/// # Safety
/// Non-null `clock` is initialized and retained. Its owner has drained/unregistered
/// the `IOProc` before reset; readers may continue, but record/reset cannot overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_callback_clock_reset(clock: *mut CallbackClock) {
    // SAFETY: initialized owner-retained storage, with callback mutation drained.
    if let Some(clock) = unsafe { clock.as_ref() } {
        // SAFETY: drained IOProc grants exclusive writer-state, not publication, access.
        unsafe { (*clock.writer.get()).primed = false };
    }
}

/// Tries exactly one snapshot; false leaves `result` untouched.
/// # Safety
/// Non-null `clock` is initialized and remains live throughout the read. Non-null
/// `result` is exclusively writable, aligned snapshot storage disjoint from clock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_callback_clock_snapshot(
    clock: *const CallbackClock,
    result: *mut CallbackClockSnapshot,
) -> bool {
    // SAFETY: only the atomic publication is accessed through this shared borrow.
    let Some(clock) = (unsafe { clock.as_ref() }) else {
        return false;
    };
    if result.is_null() {
        return false;
    }
    let Some(snapshot) = clock.publication.snapshot() else {
        return false;
    };
    // SAFETY: exclusive, aligned output storage is disjoint from the retained clock.
    unsafe { result.write(snapshot) };
    true
}

/// Reads the accumulated counter even when a coherent timestamp is unavailable.
/// # Safety
/// Non-null `clock` is initialized and retained by its native owner for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_callback_clock_discontinuities(clock: *const CallbackClock) -> u64 {
    // SAFETY: owner retains storage; only one atomic publication field is read.
    unsafe { clock.as_ref() }.map_or(0, |clock| clock.publication.discontinuities.load(SeqCst))
}

#[cfg(test)]
mod tests;
