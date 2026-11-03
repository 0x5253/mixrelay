//! Application capture ownership: one control owner, one native producer, atomic observers.
#![cfg(target_os = "macos")]

use std::{
    cell::{Cell, RefCell},
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use mixrelay_macos_transport::{
    callback_clock::{
        CallbackClock, CallbackClockSnapshot, mr_callback_clock_discontinuities,
        mr_callback_clock_snapshot,
    },
    capture_ring::{
        AudioRing, mr_audio_ring_available_frames, mr_audio_ring_destroy,
        mr_audio_ring_dropped_frames,
    },
};
use process_tap_input::{TapFormat, TapPeaks};
use process_tap_lifecycle::TapLifecycle;

mod process_tap_input;
mod process_tap_lifecycle;
mod watchdog;

#[cfg(test)]
#[cfg(not(debug_assertions))]
mod benchmark;
mod control;
mod control_trace;
mod ffi;
mod native;
#[cfg(test)]
mod tests;

/// Number of native capture owners, including sessions awaiting safe teardown.
/// Returns zero only once every admitted session released its native ownership.
#[must_use]
pub fn active_capture_owners() -> u32 {
    process_tap_lifecycle::active_owners()
}

/// One process's current use of one audio device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceUse {
    pub process_id: i32,
    pub bundle_id: String,
    pub device_uid: String,
    /// The process is playing into the device (its output side).
    pub playing: bool,
    /// The process is recording from the device (its input side).
    pub recording: bool,
}

/// Which processes are currently playing into or recording from which audio devices,
/// read from `CoreAudio` process objects (macOS 14.2 and later).
///
/// # Errors
/// Fails when the system process list cannot be read.
pub fn device_uses() -> Result<Vec<DeviceUse>, TapError> {
    native::device_uses()
}

/// Records one process's device lists as one entry per device, merging a device it both
/// plays into and records from.
fn push_device_uses(
    uses: &mut Vec<DeviceUse>,
    process_id: i32,
    bundle_id: &str,
    playing: &[String],
    recording: &[String],
) {
    for (devices, output) in [(playing, true), (recording, false)] {
        for device_uid in devices {
            match uses
                .iter_mut()
                .find(|entry| entry.process_id == process_id && entry.device_uid == *device_uid)
            {
                Some(entry) if output => entry.playing = true,
                Some(entry) => entry.recording = true,
                None => uses.push(DeviceUse {
                    process_id,
                    bundle_id: bundle_id.to_owned(),
                    device_uid: device_uid.clone(),
                    playing: output,
                    recording: !output,
                }),
            }
        }
    }
}

pub use ffi::{
    mr_process_tap_create, mr_process_tap_destroy, mr_process_tap_read, mr_process_tap_read_timed,
    mr_process_tap_refresh_format, mr_process_tap_start, mr_process_tap_stats, mr_process_tap_stop,
    mr_process_tap_take_peaks, mr_process_tap_wakeup,
};

/// Existing application-capture wire error. A nonzero code explains the failed boundary.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("application capture error {code} (OSStatus {os_status})")]
pub struct TapError {
    /// Stable `MRProcessTapErrorCode` value.
    pub code: u32,
    /// Original `OSStatus`, zero for validation/admission errors.
    pub os_status: i32,
}

impl TapError {
    const fn code(code: u32) -> Self {
        Self { code, os_status: 0 }
    }
    fn status(status: i32) -> Result<(), Self> {
        if status == 0 {
            Ok(())
        } else {
            Err(Self {
                code: 4,
                os_status: status,
            })
        }
    }
}

/// Application capture startup options, borrowed only during creation.
#[repr(C)]
#[derive(Debug)]
pub struct TapConfig {
    /// NUL-terminated UTF-8 application bundle identifier.
    pub bundle_id: *const std::ffi::c_char,
    /// Whether the selected process and owned device stream are muted while captured.
    pub mute_while_captured: bool,
    /// Power-of-two PCM capacity, from 256 through 1,048,576 frames.
    pub ring_capacity_frames: usize,
    /// Exact NUL-terminated owned device UID for mute; null for preserved playback.
    pub target_device_uid: *const std::ffi::c_char,
    /// Whole-system selection; `bundle_id` must be null and playback is preserved.
    pub system_audio: bool,
    /// Borrowed UTF-8 bundle IDs excluded from whole-system capture.
    pub excluded_bundle_ids: *const *const std::ffi::c_char,
    /// Number of entries in `excluded_bundle_ids`, at most 64.
    pub excluded_bundle_count: usize,
}

/// Identity expected by one pump read; both fences belong to the PCM producer.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct TapReadGeneration {
    pub format: u64,
    pub clock_breaks: u64,
}

/// Independently sampled diagnostic values; PCM ownership is never transferred.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct TapStats {
    /// Number of native callbacks, including gated/malformed buffers.
    pub callback_count: u64,
    /// Frames successfully stored in the capture ring.
    pub captured_frames: u64,
    /// Valid frames rejected for ring capacity.
    pub dropped_frames: u64,
    /// Currently queued frames.
    pub available_frames: usize,
    /// Immutable channel width.
    pub channels: u32,
    /// Latest validated native sample rate.
    pub sample_rate: f64,
    /// Native start has succeeded and stop/unregister has not completed.
    pub running: bool,
    /// Format fields belong to one admitted generation. Retry a false snapshot;
    /// it does not mean the capture owner has permanently stopped.
    pub format_ready: bool,
    /// PCM generations after format preparation or a reusable capture restart.
    pub format_generation: u64,
    /// Observed native timestamp discontinuities.
    pub clock_discontinuities: u64,
    /// Last coherent sample position.
    pub sample_time: f64,
    /// Last coherent host timestamp.
    pub host_time: u64,
    /// Whether that timestamp snapshot was valid.
    pub clock_valid: bool,
}

const _: () = assert!(size_of::<TapConfig>() == 56 && align_of::<TapConfig>() == 8);
const _: () = assert!(size_of::<TapError>() == 8 && align_of::<TapError>() == 4);
const _: () = assert!(size_of::<TapStats>() == 96 && align_of::<TapStats>() == 8);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum RebuildIntent {
    #[default]
    Unchanged,
    Replace,
}

#[derive(Debug, Default, Clone, Copy)]
struct ListenerReceipts {
    service_epoch: u64,
    format: bool,
    inventory: bool,
    service: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct Resources {
    tap: u32,
    aggregate: u32,
    proc: *mut std::ffi::c_void,
    listeners: ListenerReceipts,
    resume: bool,
    /// A repeated native start needs a new PCM generation, including a failed
    /// start that already called its `IOProc` before returning an error.
    start_attempted: bool,
    /// A failed native teardown must not downgrade a requested tap replacement
    /// into a format-only `IOProc` restart on the next control pass.
    rebuild: RebuildIntent,
}

#[derive(Debug)]
struct RingOwner(*mut AudioRing);

impl Drop for RingOwner {
    fn drop(&mut self) {
        // SAFETY: Session is reclaimed only after callback/listener removal and drain.
        // The consuming desktop destroy contract has already drained pump/stat readers.
        unsafe { mr_audio_ring_destroy(self.0) };
    }
}

/// Opaque capture owner. Control calls are serialized; callback/listener and meter
/// observers access only immutable fields and atomics. No whole-session mutable
/// reference or `Send`/`Sync` assertion is needed at the native callback boundary.
#[derive(Debug, Default)]
pub struct TapSession {
    timebase: OnceLock<mixrelay_realtime::Timebase>,
    retirement_queue: usize,
    resources: Cell<Resources>,
    selection: RefCell<Option<native::Selection>>,
    inventory_epoch: AtomicU64,
    service_epoch: AtomicU64,
    rebuilt_service_epoch: Cell<u64>,
    format: OnceLock<TapFormat>,
    ring: OnceLock<RingOwner>,
    lifecycle: TapLifecycle,
    peaks: TapPeaks,
    clock: CallbackClock,
    running: AtomicBool,
    /// PCM admission opens before native Start so its synchronous first callback
    /// is accepted. Stop and failed Start close it independently of native ownership.
    delivery: AtomicBool,
    sample_rate: AtomicU64,
    generation: AtomicU64,
    callbacks: AtomicU64,
    captured: AtomicU64,
    /// Native PCM evidence; meters may drain independently and cannot certify recovery.
    audible_callbacks: AtomicU64,
    /// Only the serialized control owner touches watchdog state or its diagnostic tap.
    watchdog: RefCell<watchdog::TapWatchdog>,
    active_callbacks: AtomicU32,
    active_listeners: AtomicU32,
    /// Signaled after each callback's ring write so the capture pump need not poll.
    wakeup: OnceLock<mixrelay_realtime::Notifier>,
}

impl TapSession {
    fn allowed(&self) -> bool {
        // SAFETY: immutable session borrow retains the initialized atomic gate.
        self.lifecycle.delivery_allowed()
    }

    fn delivering(&self) -> bool {
        self.delivery.load(Ordering::Acquire) && self.allowed()
    }

    fn ring(&self) -> *mut AudioRing {
        self.ring.get().map_or(ptr::null_mut(), |owner| owner.0)
    }

    fn stats(&self) -> TapStats {
        self.stats_observed(|| {})
    }

    fn stats_observed(&self, interleave: impl FnOnce()) -> TapStats {
        // The release publishing a generation follows its rate store. Acquire
        // that generation first so timed reads cannot certify a previously read rate.
        let format_generation = self.generation.load(Ordering::Acquire);
        let sample_rate = f64::from_bits(self.sample_rate.load(Ordering::Acquire));
        interleave();
        let mut clock = CallbackClockSnapshot::default();
        // SAFETY: stats readers retain the owner; all ring observations and clock
        // publication are atomic. Neither reaches control-only resources.
        let (read, clock_discontinuities, dropped_frames, available_frames) = unsafe {
            (
                mr_callback_clock_snapshot(&raw const self.clock, &raw mut clock),
                mr_callback_clock_discontinuities(&raw const self.clock),
                mr_audio_ring_dropped_frames(self.ring()),
                mr_audio_ring_available_frames(self.ring()),
            )
        };
        let mut stats = TapStats {
            callback_count: self.callbacks.load(Ordering::Relaxed),
            captured_frames: self.captured.load(Ordering::Relaxed),
            dropped_frames,
            available_frames,
            channels: self.format.get().map_or(0, |format| format.channels),
            sample_rate,
            running: self.running.load(Ordering::Acquire),
            format_ready: false,
            format_generation,
            clock_discontinuities,
            sample_time: clock.sample_time,
            host_time: clock.host_time,
            clock_valid: read && clock.valid != 0,
        };
        // A mixed observation must publish neither PCM admission nor native clock
        // credit. Keep native running separate so a transient cannot end its pump.
        let admitted = self.allowed();
        // Keep all sampled fields and admission before the final generation check.
        // A refresh certifies admission only after publishing its new generation.
        std::sync::atomic::fence(Ordering::Acquire);
        stats.format_ready =
            admitted && self.generation.load(Ordering::Relaxed) == format_generation;
        if !stats.format_ready {
            stats.clock_valid = false;
        }
        stats.clock_valid &= self.delivery.load(Ordering::Acquire);
        stats
    }
}

impl Drop for TapSession {
    fn drop(&mut self) {
        drop(self.ring.take());
        // SAFETY: only the retirement owner drops after native unregister/drain and
        // aggregate/tap destruction succeed. Admission remains held on every retry.
        self.lifecycle.release_owner();
    }
}
