//! Application capture sessions, graph handoffs, and desktop capture commands.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr::NonNull,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use mixrelay_audio_graph::{ApplicationCaptureRequest, ApplicationSelector, SourcePlaybackPolicy};
use mixrelay_audio_io::{
    ApplicationCaptureBackend, ApplicationCaptureHealth, ApplicationCaptureSession,
    AudioDeviceDescriptor, CapturableApplication, ChannelLayout, HostAudioBackendError,
    ResolvedApplicationCapture, StreamSpec,
};
use mixrelay_macos_transport::endpoint_identity::{endpoint_uid, parse_endpoint_id};
use tauri::State;
// Keep the crate exporting the native capture entry points linked into the desktop.
use mixrelay_process_tap::{self as _, TapStats as NativeStats};

#[path = "macos_audio/capture_error.rs"]
mod capture_error;
use capture_error::{CaptureOperation, CaptureProbeError, NativeError};

#[path = "macos_audio/capture_backend.rs"]
mod capture_backend;
use capture_backend::MacApplicationCaptureBackend;
#[path = "macos_audio/capture_ffi.rs"]
mod capture_ffi;
#[cfg(test)]
#[path = "macos_audio/tests.rs"]
mod tests;
use capture_ffi::{
    NativeConfig, NativeProcessTapSession, mr_process_tap_create, mr_process_tap_destroy,
    mr_process_tap_read, mr_process_tap_read_timed, mr_process_tap_refresh_format,
    mr_process_tap_start, mr_process_tap_stats, mr_process_tap_stop, mr_process_tap_take_peaks,
    mr_process_tap_visit_applications, mr_process_tap_wakeup,
};
#[path = "macos_audio/source_transport.rs"]
mod source_transport;
use source_transport::{ApplicationSourceTransport, NativeClockSnapshot};

use mixrelay_desktop_application::capture::{
    ownership as capture_transaction, reader as capture_reader,
};
#[path = "macos_audio/clock_pump.rs"]
mod clock_pump;
#[path = "macos_audio/device_activity.rs"]
mod device_activity;
pub(crate) use device_activity::DeviceActivityCache;

/// Apps currently playing into or recording from each `MixRelay` device, keyed by the
/// device's native UID. A device absent from the map has no app using it.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) fn list_device_app_activity(
    cache: State<'_, DeviceActivityCache>,
) -> Result<std::collections::BTreeMap<String, Vec<device_activity::DeviceAppActivity>>, String> {
    cache.activity().map_err(|error| error.to_string())
}

const CAPTURE_RING_CAPACITY_FRAMES: usize = mixrelay_audio_io::retention::RING_FRAMES as usize;
const ENGINE_BLOCK_FRAMES: usize = 128;
const MAXIMUM_METER_CHANNELS: usize = 64;
const PUMP_IDLE_INTERVAL: Duration = Duration::from_micros(250);

const NATIVE_OK: u32 = 0;
const NATIVE_INVALID_ARGUMENT: u32 = 1;
const NATIVE_RESOURCE_BUSY: u32 = 9;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaptureProbeStats {
    callback_count: u64,
    captured_frames: u64,
    dropped_frames: u64,
    available_frames: usize,
    channels: u32,
    sample_rate: f64,
    running: bool,
    format_ready: bool,
    format_generation: u64,
    clock_discontinuities: u64,
    sample_time: f64,
    host_time: u64,
    clock_valid: bool,
}

impl From<NativeStats> for CaptureProbeStats {
    fn from(stats: NativeStats) -> Self {
        Self {
            callback_count: stats.callback_count,
            captured_frames: stats.captured_frames,
            dropped_frames: stats.dropped_frames,
            available_frames: stats.available_frames,
            channels: stats.channels,
            sample_rate: stats.sample_rate,
            running: stats.running,
            format_ready: stats.format_ready,
            format_generation: stats.format_generation,
            clock_discontinuities: stats.clock_discontinuities,
            sample_time: stats.sample_time,
            host_time: stats.host_time,
            clock_valid: stats.clock_valid,
        }
    }
}

struct CaptureProbeSession {
    native: NonNull<NativeProcessTapSession>,
    reader_claim: AtomicBool,
    resolved: ResolvedApplicationCapture,
    #[cfg(debug_assertions)]
    debug_meter_reported: AtomicBool,
}

// The pointer owns a Core Audio session whose callback state uses C atomics.
// Lifecycle setup happens before publication; the move-only reader serializes
// pump-side format refresh and PCM consumption. Probe commands own a separate mutex.
unsafe impl Send for CaptureProbeSession {}
// SAFETY: shared access exposes atomic telemetry, never a PCM read. A non-Clone
// reader token owns the only SPSC consumer and requires mutable access to read.
unsafe impl Sync for CaptureProbeSession {}

impl CaptureProbeSession {
    /// Signaled after each tap callback's ring write; `None` means poll on a timer.
    fn wakeup(&self) -> Option<&mixrelay_realtime::Notifier> {
        // SAFETY: the notifier lives as long as this session's native owner, which this
        // borrow keeps alive.
        unsafe {
            mr_process_tap_wakeup(self.native.as_ptr())
                .cast::<mixrelay_realtime::Notifier>()
                .as_ref()
        }
    }

    fn start(bundle_id: &str, target_device_uid: Option<&str>) -> Result<Self, CaptureProbeError> {
        let session = Self::prepare(bundle_id, target_device_uid)?;
        session.start_native()?;
        Ok(session)
    }

    fn prepare(
        bundle_id: &str,
        target_device_uid: Option<&str>,
    ) -> Result<Self, CaptureProbeError> {
        Self::prepare_selector(
            &ApplicationSelector::BundleId {
                bundle_id: bundle_id.into(),
                include_descendants: false,
            },
            target_device_uid,
        )
    }

    fn prepare_selector(
        selector: &ApplicationSelector,
        target_device_uid: Option<&str>,
    ) -> Result<Self, CaptureProbeError> {
        let invalid = || {
            CaptureProbeError::new(
                CaptureOperation::Validate,
                NativeError {
                    code: NATIVE_INVALID_ARGUMENT,
                    os_status: 0,
                },
            )
        };
        let source_id =
            mixrelay_macos_transport::capture_selection::source_id(selector).ok_or_else(invalid)?;
        let (bundle, excluded) = match selector {
            ApplicationSelector::BundleId {
                bundle_id,
                include_descendants: false,
            } => (
                Some(CString::new(bundle_id.as_str()).map_err(|_| invalid())?),
                Vec::new(),
            ),
            ApplicationSelector::AllExcept { bundle_ids } => (
                None,
                bundle_ids
                    .iter()
                    .map(|id| CString::new(id.as_str()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| invalid())?,
            ),
            _ => return Err(invalid()),
        };
        let excluded_pointers = excluded.iter().map(|id| id.as_ptr()).collect::<Vec<_>>();
        let target_device_uid = target_device_uid
            .map(CString::new)
            .transpose()
            .map_err(|_| invalid())?;
        let config = NativeConfig {
            bundle_id: bundle.as_ref().map_or(std::ptr::null(), |id| id.as_ptr()),
            mute_while_captured: target_device_uid.is_some(),
            ring_capacity_frames: CAPTURE_RING_CAPACITY_FRAMES,
            target_device_uid: target_device_uid
                .as_ref()
                .map_or(std::ptr::null(), |uid| uid.as_ptr()),
            system_audio: bundle.is_none(),
            excluded_bundle_ids: excluded_pointers.as_ptr(),
            excluded_bundle_count: excluded_pointers.len(),
        };
        let mut error = NativeError::default();
        // SAFETY: `config` and its C string remain alive for the synchronous creation call.
        let native = unsafe { mr_process_tap_create(&raw const config, &raw mut error) };
        let native =
            NonNull::new(native).ok_or(CaptureProbeError::new(CaptureOperation::Prepare, error))?;
        // SAFETY: the newly created native owner remains live and exclusively controlled.
        let stats: CaptureProbeStats = unsafe { mr_process_tap_stats(native.as_ptr()) }.into();
        let Ok(channels) = u16::try_from(stats.channels) else {
            // SAFETY: no Arc/session escaped; destroy consumes the native owner.
            unsafe { mr_process_tap_destroy(native.as_ptr()) };
            return Err(CaptureProbeError::new(
                CaptureOperation::Prepare,
                NativeError {
                    code: NATIVE_INVALID_ARGUMENT,
                    os_status: 0,
                },
            ));
        };
        let channel_layout = match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            _ => ChannelLayout::Discrete,
        };
        Ok(Self {
            native,
            reader_claim: AtomicBool::new(false),
            resolved: ResolvedApplicationCapture {
                source_id,
                stream: StreamSpec {
                    channels,
                    channel_layout,
                },
            },
            #[cfg(debug_assertions)]
            debug_meter_reported: AtomicBool::new(false),
        })
    }

    fn start_native(&self) -> Result<(), CaptureProbeError> {
        // SAFETY: caller exclusively owns native control, before any pump handoff.
        let error = unsafe { mr_process_tap_start(self.native.as_ptr()) };
        if error.code == NATIVE_OK {
            Ok(())
        } else {
            Err(CaptureProbeError::new(CaptureOperation::Start, error))
        }
    }

    fn stats(&self) -> CaptureProbeStats {
        // SAFETY: the session remains owned for the duration of this call.
        unsafe { mr_process_tap_stats(self.native.as_ptr()) }.into()
    }

    fn take_native_peaks(&self, channel_count: usize) -> Vec<f32> {
        let mut peaks = [0.0_f32; MAXIMUM_METER_CHANNELS];
        // SAFETY: the destination is writable for the declared capacity and the session is alive.
        let count = unsafe {
            mr_process_tap_take_peaks(
                self.native.as_ptr(),
                peaks.as_mut_ptr(),
                channel_count.min(MAXIMUM_METER_CHANNELS),
            )
        };
        #[cfg(debug_assertions)]
        if peaks[..count].iter().any(|peak| *peak > 0.0)
            && !self.debug_meter_reported.swap(true, Ordering::Relaxed)
        {
            let stats = self.stats();
            eprintln!(
                "application meter started in desktop: running={} callbacks={} captured_frames={} dropped_frames={} available_frames={} channels={} sample_rate={} peaks={:?}",
                stats.running,
                stats.callback_count,
                stats.captured_frames,
                stats.dropped_frames,
                stats.available_frames,
                stats.channels,
                stats.sample_rate,
                &peaks[..count],
            );
        }
        peaks[..count].to_vec()
    }

    fn acquire_reader(self: &Arc<Self>) -> Result<NativeCaptureReader, String> {
        capture_reader::ExclusiveReader::acquire(Arc::clone(self))
            .map(NativeCaptureReader)
            .ok_or_else(|| "application capture already has a PCM reader".to_owned())
    }
}

impl capture_reader::ReaderOwner for CaptureProbeSession {
    fn reader_claim(&self) -> &AtomicBool {
        &self.reader_claim
    }
}

/// Telemetry keeps an Arc session; only this move-only token can consume PCM.
struct NativeCaptureReader(capture_reader::ExclusiveReader<CaptureProbeSession>);

impl NativeCaptureReader {
    fn read_timed(
        &mut self,
        destination: &mut [f32],
        times: &mut [u64],
        generation: (u64, u64),
    ) -> usize {
        let session = self.session();
        let channels = usize::from(session.resolved.stream.channels);
        if channels == 0 || destination.len() != times.len() * channels {
            return 0;
        }
        // SAFETY: this move-only reader owns both disjoint outputs and the sole ring cursor.
        unsafe {
            mr_process_tap_read_timed(
                session.native.as_ptr(),
                destination.as_mut_ptr(),
                times.as_mut_ptr(),
                times.len(),
                mixrelay_process_tap::TapReadGeneration {
                    format: generation.0,
                    clock_breaks: generation.1,
                },
            )
        }
    }
    fn session(&self) -> &CaptureProbeSession {
        self.0.owner()
    }

    fn read_interleaved(&mut self, destination: &mut [f32]) -> usize {
        let session = self.session();
        let channels = usize::from(session.resolved.stream.channels);
        if channels == 0 {
            return 0;
        }
        // SAFETY: the atomic lease admits one reader; &mut prevents overlapping
        // calls through it. Its owner Arc outlives the bounded native read.
        unsafe {
            mr_process_tap_read(
                session.native.as_ptr(),
                destination.as_mut_ptr(),
                destination.len() / channels,
            )
        }
    }
}

impl ApplicationCaptureSession for CaptureProbeSession {
    fn resolved(&self) -> &ResolvedApplicationCapture {
        &self.resolved
    }

    fn read_interleaved(
        &mut self,
        destination: &mut [f32],
    ) -> Result<usize, HostAudioBackendError> {
        let channels = usize::from(self.resolved.stream.channels);
        if channels == 0 {
            return Ok(0);
        }
        // SAFETY: &mut session excludes shared Arc readers (Arc::get_mut fails
        // while a reader owns an Arc). The destination covers maximum_frames.
        Ok(unsafe {
            mr_process_tap_read(
                self.native.as_ptr(),
                destination.as_mut_ptr(),
                destination.len() / channels,
            )
        })
    }

    fn take_peaks(&self) -> Result<Vec<f32>, HostAudioBackendError> {
        Ok(self.take_native_peaks(usize::from(self.resolved.stream.channels)))
    }

    fn health(&self) -> Result<ApplicationCaptureHealth, HostAudioBackendError> {
        let stats = self.stats();
        Ok(ApplicationCaptureHealth {
            callback_count: stats.callback_count,
            captured_frames: stats.captured_frames,
            dropped_frames: stats.dropped_frames,
            available_frames: stats.available_frames,
            running: stats.running,
        })
    }
}

impl Drop for CaptureProbeSession {
    fn drop(&mut self) {
        // SAFETY: `CaptureProbeSession` has unique ownership and the C destructor is null-safe.
        unsafe { mr_process_tap_destroy(self.native.as_ptr()) };
    }
}

struct ApplicationAudioSession {
    capture: Arc<CaptureProbeSession>,
    pump: Arc<AudioPump>,
    prepared_reader: Mutex<Option<NativeCaptureReader>>,
    playback: Option<Arc<ScopedPlayback>>,
}

/// A separate, exact-device tap suppresses playback without narrowing shared capture.
struct ScopedPlayback(Mutex<Option<CaptureProbeSession>>);

impl ScopedPlayback {
    fn start(&self) -> Result<bool, String> {
        self.refresh()?;
        let owner = self
            .0
            .lock()
            .map_err(|_| "scoped playback owner is poisoned")?;
        let capture = owner.as_ref().ok_or("scoped playback lease was revoked")?;
        if capture.stats().running {
            return Ok(false);
        }
        capture.start_native().map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn stop(&self) -> Result<(), String> {
        let owner = self
            .0
            .lock()
            .map_err(|_| "scoped playback owner is poisoned")?;
        let Some(capture) = owner.as_ref() else {
            return Ok(());
        };
        if !capture.stats().running {
            return Ok(());
        }
        // SAFETY: this mutex is the sole control owner; no ring reader uses this tap.
        let error = unsafe { mr_process_tap_stop(capture.native.as_ptr()) };
        if error.code == NATIVE_OK {
            Ok(())
        } else {
            Err(CaptureProbeError::new(CaptureOperation::Stop, error).to_string())
        }
    }

    fn refresh(&self) -> Result<(), String> {
        let mut owner = self
            .0
            .lock()
            .map_err(|_| "scoped playback owner is poisoned")?;
        let capture = owner.as_ref().ok_or("scoped playback lease was revoked")?;
        // SAFETY: only this serialized owner refreshes the scoped tap.
        let error = unsafe { mr_process_tap_refresh_format(capture.native.as_ptr()) };
        if error.code == NATIVE_OK {
            Ok(())
        } else {
            // Destroy transfers this exact tap to native retirement; retaining
            // the empty owner prevents later ACK retries from restoring its lease.
            drop(owner.take());
            Err(CaptureProbeError::new(CaptureOperation::Refresh, error).to_string())
        }
    }
}

struct ActivePlayback(Option<Arc<ScopedPlayback>>);

impl Drop for ActivePlayback {
    fn drop(&mut self) {
        if let Some(playback) = &self.0
            && let Err(error) = playback.stop()
        {
            tracing::error!(%error, "scoped playback restoration failed; native owner retained for teardown");
        }
    }
}

struct AudioPump {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    handoff: Arc<capture_transaction::CaptureHandoff<NativeCaptureReader>>,
    playback: Arc<capture_transaction::CaptureHandoff<Option<Arc<ScopedPlayback>>>>,
}

#[derive(Clone, Copy)]
struct PumpControl<'a> {
    stop: &'a AtomicBool,
    capture: &'a capture_transaction::CaptureHandoff<NativeCaptureReader>,
    playback: &'a capture_transaction::CaptureHandoff<Option<Arc<ScopedPlayback>>>,
}

impl AudioPump {
    fn start(source_id: &str, capture: NativeCaptureReader, channels: u16) -> Result<Self, String> {
        let transport = ApplicationSourceTransport::connect(source_id, channels)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let channel_count = usize::from(channels);
        let handoff = Arc::new(capture_transaction::CaptureHandoff::default());
        let worker_handoff = Arc::clone(&handoff);
        let playback = Arc::new(capture_transaction::CaptureHandoff::default());
        let worker_playback = Arc::clone(&playback);
        let worker = std::thread::Builder::new()
            .name(format!("mixrelay-source-{source_id}"))
            .spawn(move || {
                run_audio_pump(
                    capture,
                    transport,
                    channel_count,
                    PumpControl {
                        stop: &worker_stop,
                        capture: &worker_handoff,
                        playback: &worker_playback,
                    },
                );
            })
            .map_err(|error| format!("could not start application source pump: {error}"))?;
        Ok(Self {
            stop,
            worker: Some(worker),
            handoff,
            playback,
        })
    }
}

impl Drop for AudioPump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_audio_pump(
    capture: NativeCaptureReader,
    transport: ApplicationSourceTransport,
    channels: usize,
    control: PumpControl<'_>,
) {
    if let Err(error) = clock_pump::run_with_handoff(capture, transport, channels, control) {
        tracing::error!(%error, "application source clock bridge stopped");
    }
}

unsafe extern "C" fn collect_application(
    context: *mut c_void,
    bundle_id: *const c_char,
    display_name: *const c_char,
    icon_data_url: *const c_char,
    process_id: i32,
) {
    if context.is_null() || bundle_id.is_null() || display_name.is_null() {
        return;
    }
    // SAFETY: the native enumerator keeps both pointers valid for this synchronous callback.
    let applications = unsafe { &mut *context.cast::<Vec<CapturableApplication>>() };
    // SAFETY: Core Audio bundle IDs are passed as null-terminated UTF-8 for this callback.
    let bundle_id = unsafe { CStr::from_ptr(bundle_id) };
    let display_name = unsafe { CStr::from_ptr(display_name) };
    let icon_data_url = if icon_data_url.is_null() {
        None
    } else {
        // SAFETY: the native enumerator keeps this optional pointer valid for the callback.
        unsafe { CStr::from_ptr(icon_data_url) }
            .to_str()
            .ok()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    if let (Ok(bundle_id), Ok(display_name), Ok(process_id)) = (
        bundle_id.to_str(),
        display_name.to_str(),
        u32::try_from(process_id),
    ) && !bundle_id.is_empty()
        && !display_name.is_empty()
    {
        applications.push(CapturableApplication {
            bundle_id: bundle_id.to_owned(),
            display_name: display_name.to_owned(),
            icon_data_url,
            process_id,
            selector: ApplicationSelector::BundleId {
                bundle_id: bundle_id.to_owned(),
                include_descendants: false,
            },
        });
    }
}

#[derive(Default)]
pub(crate) struct CaptureProbeState {
    session: Mutex<Option<CaptureProbeSession>>,
}

pub(crate) struct PreparedApplicationCaptures {
    set: capture_transaction::CaptureSet<ApplicationAudioSession>,
}

#[derive(Default)]
pub(crate) struct ApplicationCaptureState {
    owners: Mutex<capture_transaction::CaptureOwners<ApplicationAudioSession>>,
    backend: MacApplicationCaptureBackend,
    pub(crate) monitor_active: AtomicBool,
    /// Counts engines; observations taken before a release describe the lost engine.
    engine_generation: AtomicU64,
}

impl ApplicationCaptureState {
    pub(crate) fn reserve(
        &self,
        manifest: &mixrelay_control_protocol::ApplicationCaptureManifest,
    ) -> Result<(), String> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        owners
            .ensure_reservable()
            .map_err(|error| error.to_string())?;
        let retained = owners.retained().map_err(|error| error.to_string())?;
        let set = capture_transaction::prepare(
            manifest,
            owners.latest(),
            &retained,
            |id, request, previous| prepare_application_audio(&self.backend, id, request, previous),
        )
        .map_err(|error| error.to_string())?;
        owners.reserve(set).map_err(|error| error.to_string())
    }

    pub(crate) fn accept_reserved(&self) -> Result<(), String> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        let retired = owners
            .accept_reserved()
            .map_err(|error| error.to_string())?;
        drop(owners);
        drop(retired);
        Ok(())
    }

    pub(crate) fn reject_reserved(&self) -> Result<(), String> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        let retired = owners.reject_reserved();
        drop(owners);
        drop(retired);
        Ok(())
    }

    pub(crate) fn prepare(
        &self,
        manifest: &mixrelay_control_protocol::ApplicationCaptureManifest,
    ) -> Result<PreparedApplicationCaptures, String> {
        let owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        let retained = owners.retained().map_err(|error| error.to_string())?;
        let set = capture_transaction::prepare(
            manifest,
            owners.latest(),
            &retained,
            |id, request, previous| prepare_application_audio(&self.backend, id, request, previous),
        )
        .map_err(|error| error.to_string())?;
        Ok(PreparedApplicationCaptures { set })
    }

    pub(crate) fn stage(&self, prepared: PreparedApplicationCaptures) -> Result<(), String> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        let retired = owners
            .stage(prepared.set)
            .map_err(|error| error.to_string())?;
        drop(owners);
        drop(retired);
        Ok(())
    }

    pub(crate) fn acknowledge(&self, revision: u64) -> Result<(), String> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        let retired = owners
            .acknowledge_started(
                revision,
                |session| {
                    let Some(playback) = &session.playback else {
                        return Ok(None);
                    };
                    playback
                        .start()
                        .map(|started| started.then(|| Arc::clone(playback)))
                },
                |owner| owner.stop(),
                |session| {
                    let reader = session
                        .prepared_reader
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some(reader) = reader {
                        session.pump.handoff.publish(reader);
                    }
                    session.pump.playback.publish(session.playback.clone());
                },
            )
            .map_err(|error| error.to_string())?;
        drop(owners);
        // Destruction joins native pump workers only on this blocking control thread.
        drop(retired);
        Ok(())
    }

    /// Drops every owner once the engine is lost: their transports map that engine's
    /// memory, and its successor publishes its own revisions. Joins pump workers, so
    /// call it on a blocking thread.
    pub(crate) fn release_all(&self) -> Result<(), String> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())?;
        self.engine_generation.fetch_add(1, Ordering::AcqRel);
        let released = std::mem::take(&mut *owners);
        drop(owners);
        drop(released);
        Ok(())
    }

    pub(crate) fn engine_generation(&self) -> u64 {
        self.engine_generation.load(Ordering::Acquire)
    }

    pub(crate) fn has_pending(&self) -> Result<bool, String> {
        self.owners
            .lock()
            .map(|owners| owners.has_unacknowledged())
            .map_err(|_| "application capture state is poisoned".to_owned())
    }

    pub(crate) fn requires_manifest(&self, submitted_revision: u64) -> Result<bool, String> {
        self.owners
            .lock()
            .map(|owners| owners.requires_manifest(submitted_revision))
            .map_err(|_| "application capture state is poisoned".to_owned())
    }
}

fn prepare_application_audio(
    backend: &MacApplicationCaptureBackend,
    node_id: &mixrelay_audio_graph::NodeId,
    request: &ApplicationCaptureRequest,
    previous: Option<&Arc<ApplicationAudioSession>>,
) -> Result<Arc<ApplicationAudioSession>, String> {
    if previous
        .is_some_and(|session| session.capture.resolved.stream.channels != request.stream.channels)
    {
        return Err("live application transport channel replacement is unsupported".to_owned());
    }
    let playback = if request.source_playback == SourcePlaybackPolicy::MuteWhileCaptured {
        let ApplicationSelector::BundleId {
            bundle_id,
            include_descendants: false,
        } = &request.selector
        else {
            return Err("scoped playback requires an exact bundle selector".into());
        };
        let id = request
            .forwarding_endpoint
            .as_ref()
            .and_then(|id| parse_endpoint_id(id.as_str()))
            .ok_or("forwarding requires an exact MixRelay endpoint identity")?;
        Some(Arc::new(ScopedPlayback(Mutex::new(Some(
            CaptureProbeSession::prepare(bundle_id, Some(&endpoint_uid(id)))
                .map_err(|error| error.to_string())?,
        )))))
    } else {
        None
    };
    if let Some(previous) = previous.filter(|previous| {
        mixrelay_macos_transport::capture_selection::source_id(&request.selector)
            .is_some_and(|id| id == previous.capture.resolved.source_id)
    }) {
        return Ok(Arc::new(ApplicationAudioSession {
            capture: Arc::clone(&previous.capture),
            pump: Arc::clone(&previous.pump),
            prepared_reader: Mutex::new(None),
            playback,
        }));
    }
    let mut preserved = request.clone();
    preserved.source_playback = SourcePlaybackPolicy::Preserve;
    preserved.forwarding_endpoint = None;
    let session = backend
        .open(&preserved)
        .map_err(|error| error.to_string())?;
    let stats = session.stats();
    if stats.channels != u32::from(request.stream.channels) {
        return Err("application capture channel count does not match the graph".to_owned());
    }
    clock_pump::native_rate(stats.sample_rate)?;
    let capture = Arc::new(session);
    // A replacement must reserve its sole reader before remote acceptance;
    // committing only moves an existing token and cannot fail acquisition.
    let reader = capture.acquire_reader()?;
    // A second connection to the same source would steal its producer lease
    // before HTTP acceptance. Keep the transport and its single reader alive;
    // only commit publishes the prepared tap to that pump's control mailbox.
    let (pump, prepared_reader) = if let Some(previous) = previous {
        (Arc::clone(&previous.pump), Some(reader))
    } else {
        (
            Arc::new(AudioPump::start(
                node_id.as_str(),
                reader,
                request.stream.channels,
            )?),
            None,
        )
    };
    Ok(Arc::new(ApplicationAudioSession {
        capture,
        pump,
        prepared_reader: Mutex::new(prepared_reader),
        playback,
    }))
}

#[tauri::command]
pub(crate) fn list_capturable_applications() -> Result<Vec<CapturableApplication>, String> {
    let mut applications: Vec<CapturableApplication> = Vec::new();
    // SAFETY: enumeration and callbacks are synchronous and `applications` outlives the call.
    let error = unsafe {
        mr_process_tap_visit_applications(Some(collect_application), (&raw mut applications).cast())
    };
    if error.code != NATIVE_OK {
        return Err(CaptureProbeError::new(CaptureOperation::Enumerate, error).to_string());
    }
    applications.sort_unstable_by(|left, right| {
        left.bundle_id
            .cmp(&right.bundle_id)
            .then(left.process_id.cmp(&right.process_id))
    });
    applications.dedup_by(|right, left| right.bundle_id == left.bundle_id);
    applications.sort_unstable_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.bundle_id.cmp(&right.bundle_id))
    });
    Ok(applications)
}

#[tauri::command]
pub(crate) fn list_audio_devices() -> Result<Vec<AudioDeviceDescriptor>, String> {
    let mut devices = mixrelay_macos_audio_io::devices().map_err(|error| error.to_string())?;
    devices.sort_unstable_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.uid.cmp(&right.uid))
    });
    Ok(devices)
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri deserializes owned command arguments and State extractors"
)]
pub(crate) fn start_capture_probe(
    bundle_id: String,
    mute_while_captured: bool,
    state: State<'_, CaptureProbeState>,
) -> Result<CaptureProbeStats, String> {
    if mute_while_captured {
        return Err(
            "application source mute requires a verified MixRelay-owned forwarding endpoint"
                .to_owned(),
        );
    }
    let mut session = state
        .session
        .lock()
        .map_err(|_| "application capture state is poisoned".to_owned())?;
    if session.is_some() {
        return Err("an application capture probe is already running".to_owned());
    }
    let capture =
        CaptureProbeSession::start(&bundle_id, None).map_err(|error| error.to_string())?;
    let snapshot = capture.stats();
    *session = Some(capture);
    Ok(snapshot)
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command State is an owned request extractor"
)]
pub(crate) fn capture_probe_stats(
    state: State<'_, CaptureProbeState>,
) -> Result<CaptureProbeStats, String> {
    let session = state
        .session
        .lock()
        .map_err(|_| "application capture state is poisoned".to_owned())?;
    session
        .as_ref()
        .map(CaptureProbeSession::stats)
        .ok_or_else(|| "no application capture probe is running".to_owned())
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command State is an owned request extractor"
)]
pub(crate) fn stop_capture_probe(state: State<'_, CaptureProbeState>) -> Result<(), String> {
    let mut session = state
        .session
        .lock()
        .map_err(|_| "application capture state is poisoned".to_owned())?;
    session
        .take()
        .map(drop)
        .ok_or_else(|| "no application capture probe is running".to_owned())
}
