//! Lifetime-bounded C entry points used by the desktop capture owner.

use super::{
    RingOwner, TapConfig, TapError, TapSession, TapStats,
    native::{self, CoreAudio, Native, Timestamp},
};
use crate::control_trace::{self, Procedure};
use crate::process_tap_input::{ingest, input_frames};
use mixrelay_macos_transport::{
    audio_buffer::AudioBufferList,
    callback_clock::mr_callback_clock_record_values,
    capture_ring::{
        mr_audio_ring_create, mr_audio_ring_read_interleaved, mr_audio_ring_read_timed,
    },
};
use std::{
    alloc::{Layout, alloc},
    ffi::{CStr, c_void},
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

pub(super) struct Active<'a>(&'a AtomicU32);
impl<'a> Active<'a> {
    pub(super) fn enter(counter: &'a AtomicU32) -> Self {
        counter.fetch_add(1, Ordering::Acquire);
        Self(counter)
    }
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

pub(super) unsafe extern "C" fn io_proc(
    _: u32,
    _: *const Timestamp,
    input: *const AudioBufferList,
    time: *const Timestamp,
    _: *mut AudioBufferList,
    _: *const Timestamp,
    context: *mut c_void,
) -> i32 {
    // SAFETY: native registration retains the heap-pinned owner until unregister
    // succeeds and this active guard drains. No control-only Cell is accessed here.
    let session = unsafe { &*context.cast::<TapSession>() };
    let _active = Active::enter(&session.active_callbacks);
    session.callbacks.fetch_add(1, Ordering::Relaxed);
    if session.delivering()
        && let Some(format) = session.format.get()
    {
        // SAFETY: ring/format are installed before IOProc registration and remain
        // immutable; CoreAudio owns readable buffer extents during this callback.
        // SAFETY: native timestamp is readable for the duration of this callback.
        let time = unsafe { time.as_ref() };
        let first = time
            .filter(|time| time.flags & 3 == 3)
            .and_then(|time| {
                session
                    .timebase
                    .get()
                    .map(|base| base.nanoseconds(time.host))
            })
            .unwrap_or(0);
        let scalar = time
            .filter(|time| {
                time.flags & 4 != 0 && time.rate_scalar.is_finite() && time.rate_scalar > 0.0
            })
            .map_or(1.0, |time| time.rate_scalar);
        // Geometry is immutable, but a drained format refresh publishes a new
        // nominal rate before reopening callback admission.
        let native_rate = f64::from_bits(session.sample_rate.load(Ordering::Acquire));
        // Clock discontinuities become visible before their PCM can be published.
        // SAFETY: the same immutable CoreAudio metadata remains live through ingest.
        let frames = unsafe { input_frames(format, input) };
        // SAFETY: this is the sole serialized clock writer; other observers use atomics.
        unsafe {
            mr_callback_clock_record_values(
                ptr::from_ref(&session.clock).cast_mut(),
                time.map_or(0.0, |time| time.sample),
                time.map_or(0, |time| time.host),
                time.is_some_and(|time| time.flags & 3 == 3),
                frames as u64,
            );
        };
        // SAFETY: prepared ring/format and native PCM remain live during this callback.
        let result = unsafe {
            ingest(
                format,
                input,
                session.ring(),
                &session.peaks,
                (first, scalar * 1e9 / native_rate),
            )
        };
        session
            .captured
            .fetch_add(result.written as u64, Ordering::Relaxed);
        if result.audible {
            // The control watchdog needs only progress, not a coherent PCM snapshot.
            // Ring overflow must not turn a live producer into apparent silence.
            session.audible_callbacks.fetch_add(1, Ordering::Relaxed);
        }
        // After the ring write and its counters: the pump rechecks published state.
        if let Some(wakeup) = session.wakeup.get() {
            wakeup.notify();
        }
    }
    0
}

fn initialize(session: &TapSession, config: &TapConfig) -> Result<(), TapError> {
    native::add_service_listener(session)?;
    let mut selection = native::Selection::new(config, &CoreAudio)?;
    if selection.fallback {
        native::add_inventory_listener(session)?;
        let inventory_epoch = session.inventory_epoch.load(Ordering::Acquire);
        selection.selected = selection.processes(&CoreAudio)?;
        if selection.exclusions.is_none() && selection.selected.is_empty() {
            return Err(TapError::code(3));
        }
        if session.inventory_epoch.load(Ordering::Acquire) != inventory_epoch {
            return Err(TapError::code(9));
        }
    }
    initialize_selected(session, selection, config.ring_capacity_frames)
}

fn initialize_selected(
    session: &TapSession,
    selection: native::Selection,
    capacity: usize,
) -> Result<(), TapError> {
    let description = native::Description::create(&selection)?;
    let _previous = session.selection.replace(Some(selection));
    description.tap(session)?;
    let format = control_trace::run(Procedure::ReadTapFormat, || {
        CoreAudio.format(session.resources.get().tap)
    })?;
    if !format.supported() {
        return Err(TapError::code(5));
    }
    let rate = format.sample_rate;
    let channels = format.channels;
    session.format.set(format).map_err(|_| TapError::code(1))?;
    let ring = mr_audio_ring_create(capacity, channels);
    if ring.is_null() {
        return Err(TapError::code(6));
    }
    session
        .ring
        .set(RingOwner(ring))
        .map_err(|_| TapError::code(1))?;
    session.sample_rate.store(rate.to_bits(), Ordering::Release);
    session.generation.store(1, Ordering::Release);
    native::add_listener(session)?;
    description.aggregate(session)?;
    CoreAudio.register(session)?;
    Ok(())
}

// SAFETY: configuration and its C string have been retained by the create entry.
unsafe fn create(config: &TapConfig) -> Result<*mut TapSession, TapError> {
    if !(256..=1 << 20).contains(&config.ring_capacity_frames)
        || !config.ring_capacity_frames.is_power_of_two()
        || config.mute_while_captured == config.target_device_uid.is_null()
    {
        return Err(TapError::code(1));
    }
    // SAFETY: this entry retains the configuration and all borrowed strings.
    unsafe { validate_selector(config) }?;
    if !config.target_device_uid.is_null() {
        // SAFETY: caller retains this NUL-terminated string through startup.
        let uid = unsafe { CStr::from_ptr(config.target_device_uid) };
        if !native::owned_endpoint_uid(uid) {
            return Err(TapError::code(1));
        }
    }
    let pointer = allocate_session()?;
    // SAFETY: initialized owner remains stable through every native registration.
    let session = unsafe { &*pointer };
    if let Err(error) = initialize(session, config) {
        // SAFETY: startup transfers its partial resource ownership to async cleanup.
        unsafe { mr_process_tap_destroy(pointer) };
        return Err(error);
    }
    Ok(pointer)
}

unsafe fn validate_selector(config: &TapConfig) -> Result<(), TapError> {
    if config.system_audio {
        if !config.bundle_id.is_null()
            || config.mute_while_captured
            || !config.target_device_uid.is_null()
            || config.excluded_bundle_count > 64
            || (config.excluded_bundle_count != 0 && config.excluded_bundle_ids.is_null())
        {
            return Err(TapError::code(1));
        }
        for index in 0..config.excluded_bundle_count {
            // SAFETY: caller retains the complete pointer array through this call.
            let id = unsafe { *config.excluded_bundle_ids.add(index) };
            if id.is_null() {
                return Err(TapError::code(1));
            }
            // SAFETY: each non-null entry points to a retained C string.
            let id = unsafe { CStr::from_ptr(id) };
            if id.is_empty() || id.to_str().is_err() {
                return Err(TapError::code(1));
            }
        }
        return Ok(());
    }
    if config.bundle_id.is_null() || config.excluded_bundle_count != 0 {
        return Err(TapError::code(1));
    }
    // SAFETY: caller retains the complete NUL-terminated identifier during startup.
    let bundle = unsafe { CStr::from_ptr(config.bundle_id) };
    if bundle.is_empty() || bundle.to_str().is_err() {
        return Err(TapError::code(1));
    }
    Ok(())
}

fn allocate_session() -> Result<*mut TapSession, TapError> {
    let retirement_queue = native::prepare_cleanup()?;
    // SAFETY: fresh global allocation for exactly one correctly aligned owner.
    let pointer = unsafe { alloc(Layout::new::<TapSession>()) }
        .cast::<c_void>()
        .cast::<TapSession>();
    if pointer.is_null() {
        return Err(TapError::code(6));
    }
    // SAFETY: unpublished allocation is exclusively initialized before any borrowing.
    let mut owner = TapSession::default();
    let _ = owner.timebase.set(mixrelay_realtime::Timebase::current());
    owner.retirement_queue = retirement_queue;
    // Without a notifier the pump falls back to timed waits; capture still works.
    if let Ok(notifier) = mixrelay_realtime::Notifier::new() {
        let _ = owner.wakeup.set(notifier);
    }
    unsafe { pointer.write(owner) };
    // SAFETY: initialized owner remains stable through every native registration.
    let session = unsafe { &*pointer };
    // SAFETY: gate is unpublished and acquired by its sole startup control owner.
    if !session.lifecycle.acquire_owner() {
        // SAFETY: no native references exist on failed admission.
        drop(unsafe { Box::from_raw(pointer) });
        return Err(TapError::code(9));
    }
    Ok(pointer)
}

/// A diagnostic never suppresses playback or feeds an engine ring. Its small ring
/// may overflow; independent callback observations still certify native audio.
pub(super) fn create_diagnostic(
    selection: &native::Selection,
) -> Result<*mut TapSession, TapError> {
    let pointer = allocate_session()?;
    // SAFETY: allocation remains pinned through preparation and asynchronous retirement.
    let session = unsafe { &*pointer };
    let mut selection = selection.clone();
    selection.mute = false;
    let result =
        initialize_selected(session, selection, 256).and_then(|()| session.start(&CoreAudio));
    if let Err(error) = result {
        // SAFETY: no diagnostic owner escaped; teardown retains all partial handles.
        unsafe { mr_process_tap_destroy(pointer) };
        return Err(error);
    }
    Ok(pointer)
}

/// The session's wakeup, signaled after each callback's ring write, or null when the
/// session has none. Borrowed: valid until `mr_process_tap_destroy`.
/// # Safety
/// `session` is null or a live session returned by `mr_process_tap_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_wakeup(session: *const TapSession) -> *const c_void {
    // SAFETY: the caller passes null or a live session.
    unsafe { session.as_ref() }
        .and_then(|session| session.wakeup.get())
        .map_or(ptr::null(), |wakeup| ptr::from_ref(wakeup).cast())
}

/// Creates and owns a prepared tap. Failure returns null and its original error;
/// partially created native resources remain owned by asynchronous cleanup.
/// # Safety
/// Config, selector pointer arrays and their NUL-terminated UTF-8 strings remain
/// readable during this call. Arrays are aligned and contain the declared count.
/// Optional error points to exclusively writable `TapError` storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_create(
    config: *const TapConfig,
    error: *mut TapError,
) -> *mut TapSession {
    // SAFETY: caller retains the borrowed configuration until creation returns.
    let result = match unsafe { config.as_ref() } {
        Some(config) => unsafe { create(config) },
        None => Err(TapError::code(1)),
    };
    // SAFETY: optional output is exclusive and correctly sized by the C caller.
    if let Some(error) = unsafe { error.as_mut() } {
        *error = result.as_ref().err().copied().unwrap_or_default();
    }
    result.unwrap_or(ptr::null_mut())
}

/// Starts the prepared native registration; null is invalid.
/// # Safety
/// Session is owned and control calls are serialized with read/refresh/stop/destroy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_start(session: *mut TapSession) -> TapError {
    // SAFETY: control retains allocation; shared borrow does not alias callback writer fields.
    unsafe { session.as_ref() }.map_or(TapError::code(1), |session| {
        session.start(&CoreAudio).err().unwrap_or_default()
    })
}

/// Stops native delivery without retiring the reusable session; null is invalid.
/// # Safety
/// Session is owned and control calls are serialized with read/refresh/start/destroy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_stop(session: *mut TapSession) -> TapError {
    // SAFETY: control retains allocation through this native stop operation.
    unsafe { session.as_ref() }.map_or(TapError::code(1), |session| {
        session.stop(&CoreAudio).err().unwrap_or_default()
    })
}

/// Reconciles native format and pre-bundle-selector app identity off the audio callback.
/// Incompatible geometry stays gated; an absent selected app remains pending and silent.
/// # Safety
/// Session is owned and control calls are serialized with PCM read/start/stop/destroy.
/// Atomic stat and peak observers may run concurrently while retaining owner lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_refresh_format(session: *mut TapSession) -> TapError {
    // SAFETY: refresh retains allocation; callbacks access no control resource cells.
    unsafe { session.as_ref() }.map_or(TapError::code(1), |session| {
        session.refresh(&CoreAudio).err().unwrap_or_default()
    })
}

/// Consumes the owner without blocking; failures retain all unreleased resources for retry.
/// # Safety
/// Pump/control/stat/peak users have drained and surrendered ownership before destroy.
/// Native callbacks/listeners may remain: cleanup owns their drain. No later pointer use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_destroy(pointer: *mut TapSession) {
    // SAFETY: destroying owner retains allocation until queue transfer.
    let Some(session) = (unsafe { pointer.as_ref() }) else {
        return;
    };
    // SAFETY: atomic retire assigns at most one cleanup owner for this live allocation.
    if session.lifecycle.retire() {
        native::retire(pointer, false);
    }
}

/// Reads queued PCM for the sole pump consumer; gated/null sessions return zero.
/// # Safety
/// Session is retained and this call is serialized with refresh/control and other reads.
/// Destination owns `maximum_frames * channels` writable floats, disjoint from session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_read(
    session: *mut TapSession,
    destination: *mut f32,
    maximum_frames: usize,
) -> usize {
    // SAFETY: pump retains its session until the consumer read ends.
    let Some(session) = (unsafe { session.as_ref() }) else {
        return 0;
    };
    if !session.delivering() {
        return 0;
    }
    // SAFETY: pump owns exclusive output and the sole consumer cursor.
    unsafe { mr_audio_ring_read_interleaved(session.ring(), destination, maximum_frames) }
}

/// Reads PCM and its original per-frame monotonic nanosecond coordinates.
/// # Safety
/// Same session/sole-consumer contract as read; times owns `maximum_frames` cells.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_read_timed(
    session: *mut TapSession,
    destination: *mut f32,
    times: *mut u64,
    maximum_frames: usize,
    expected: super::TapReadGeneration,
) -> usize {
    // SAFETY: pump retains the session and both disjoint writable output extents.
    let Some(session) = (unsafe { session.as_ref() }) else {
        return 0;
    };
    let current = || {
        session.delivering()
            && session.generation.load(Ordering::Acquire) == expected.format
            && unsafe {
                mixrelay_macos_transport::callback_clock::mr_callback_clock_discontinuities(
                    &raw const session.clock,
                )
            } == expected.clock_breaks
    };
    if !current() {
        return 0;
    }
    // SAFETY: one pump owns this ring's consumer.
    let frames =
        unsafe { mr_audio_ring_read_timed(session.ring(), destination, times, maximum_frames) };
    if !current() {
        let channels = session
            .format
            .get()
            .map_or(0, |format| format.channels as usize);
        // SAFETY: the successfully copied PCM/time extents still belong to this pump.
        unsafe {
            ptr::write_bytes(destination, 0, frames * channels);
            ptr::write_bytes(times, 0, frames);
        }
        return 0;
    }
    frames
}

/// Drains channel peaks independently of PCM; invalid pointers/capacity return zero.
/// # Safety
/// Observer retains session lifetime and exclusively owns `maximum_channels` output floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_take_peaks(
    session: *mut TapSession,
    destination: *mut f32,
    maximum_channels: usize,
) -> usize {
    // SAFETY: observer holds the application session lifetime token.
    let Some(session) = (unsafe { session.as_ref() }) else {
        return 0;
    };
    let channels = session
        .format
        .get()
        .map_or(0, |format| format.channels as usize);
    if destination.is_null() {
        return 0;
    }
    let channels = channels.min(maximum_channels);
    // SAFETY: output capacity bounded by both caller's extent and immutable channel width.
    session
        .peaks
        .take(unsafe { std::slice::from_raw_parts_mut(destination, channels) });
    channels
}

/// Returns atomic capture observations; null yields an empty snapshot.
/// # Safety
/// Observer retains the session through this call and never overlaps its final destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_process_tap_stats(session: *const TapSession) -> TapStats {
    // SAFETY: immutable fields and atomics only; no whole-session mutable borrow exists.
    unsafe { session.as_ref() }.map_or_else(TapStats::default, TapSession::stats)
}
