//! A Core Audio process tap: native ownership, its single PCM reader and its telemetry.
use super::CAPTURE_RING_CAPACITY_FRAMES;
use super::capture_error::{
    CaptureOperation, CaptureProbeError, NATIVE_INVALID_ARGUMENT, NATIVE_OK, NATIVE_RESOURCE_BUSY,
};
use super::clock_pump::ports::{CaptureTap, FormatState, TapWake};
use mixrelay_audio_graph::ApplicationSelector;
use mixrelay_audio_io::{
    ApplicationCaptureHealth, ApplicationCaptureSession, ChannelLayout, HostAudioBackendError,
    ResolvedApplicationCapture, StreamSpec,
};
use mixrelay_desktop_core::capture::reader::{ExclusiveReader, ReaderOwner};
use mixrelay_macos_process_tap::{
    TapConfig, TapError, TapReadGeneration, TapSession, TapStats, mr_process_tap_create,
    mr_process_tap_destroy, mr_process_tap_read, mr_process_tap_read_timed,
    mr_process_tap_refresh_format, mr_process_tap_start, mr_process_tap_stats, mr_process_tap_stop,
    mr_process_tap_take_peaks, mr_process_tap_wakeup,
};
use mixrelay_realtime::{Notifier, host_time};
use std::{
    ffi::{CString, c_char},
    ptr::NonNull,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

const MAXIMUM_METER_CHANNELS: usize = 64;

/// The owned C strings a native tap is created from.
#[derive(Debug, PartialEq, Eq)]
struct TapTarget {
    /// `None` captures system audio.
    bundle_id: Option<CString>,
    excluded_bundle_ids: Vec<CString>,
    /// The `MixRelay` device whose playback the tap mutes, if any.
    device_uid: Option<CString>,
}

impl TapTarget {
    /// `None` for selectors a native tap cannot express, or text containing NUL.
    fn new(selector: &ApplicationSelector, device_uid: Option<&str>) -> Option<Self> {
        let (bundle_id, excluded_bundle_ids) = match selector {
            ApplicationSelector::BundleId {
                bundle_id,
                include_descendants: false,
            } => (Some(CString::new(bundle_id.as_str()).ok()?), Vec::new()),
            ApplicationSelector::AllExcept { bundle_ids } => (
                None,
                bundle_ids
                    .iter()
                    .map(|id| CString::new(id.as_str()))
                    .collect::<Result<_, _>>()
                    .ok()?,
            ),
            _ => return None,
        };
        let device_uid = device_uid.map(CString::new).transpose().ok()?;
        Some(Self {
            bundle_id,
            excluded_bundle_ids,
            device_uid,
        })
    }

    fn create(&self) -> Result<NativeTap, CaptureProbeError> {
        let excluded = self
            .excluded_bundle_ids
            .iter()
            .map(|id| id.as_ptr())
            .collect::<Vec<_>>();
        let config = TapConfig {
            bundle_id: optional_pointer(self.bundle_id.as_ref()),
            mute_while_captured: self.device_uid.is_some(),
            ring_capacity_frames: CAPTURE_RING_CAPACITY_FRAMES,
            target_device_uid: optional_pointer(self.device_uid.as_ref()),
            system_audio: self.bundle_id.is_none(),
            excluded_bundle_ids: excluded.as_ptr(),
            excluded_bundle_count: excluded.len(),
        };
        let mut error = TapError::default();
        // SAFETY: `config` and its C strings remain alive for the synchronous creation call.
        let native = unsafe { mr_process_tap_create(&raw const config, &raw mut error) };
        NonNull::new(native)
            .map(NativeTap)
            .ok_or(CaptureProbeError::new(CaptureOperation::Prepare, error))
    }
}

fn optional_pointer(value: Option<&CString>) -> *const c_char {
    value.map_or(std::ptr::null(), |value| value.as_ptr())
}

/// The stream a tap with this many channels delivers; `None` beyond `u16`.
fn stream_spec(channels: u32) -> Option<StreamSpec> {
    let channels = u16::try_from(channels).ok()?;
    let channel_layout = match channels {
        1 => ChannelLayout::Mono,
        2 => ChannelLayout::Stereo,
        _ => ChannelLayout::Discrete,
    };
    Some(StreamSpec {
        channels,
        channel_layout,
    })
}

/// Owns one native tap; dropping it hands the tap to native retirement.
struct NativeTap(NonNull<TapSession>);

impl NativeTap {
    fn as_ptr(&self) -> *mut TapSession {
        self.0.as_ptr()
    }

    fn stats(&self) -> TapStats {
        // SAFETY: the native owner is alive for the duration of this call.
        unsafe { mr_process_tap_stats(self.as_ptr()) }
    }
}

impl Drop for NativeTap {
    fn drop(&mut self) {
        // SAFETY: this owner is unique and the C destructor is null-safe.
        unsafe { mr_process_tap_destroy(self.as_ptr()) };
    }
}

fn checked(operation: CaptureOperation, error: TapError) -> Result<(), CaptureProbeError> {
    if error.code == NATIVE_OK {
        Ok(())
    } else {
        Err(CaptureProbeError::new(operation, error))
    }
}

pub(super) struct ProcessTap {
    native: NativeTap,
    reader_claim: AtomicBool,
    pub(super) resolved: ResolvedApplicationCapture,
    #[cfg(debug_assertions)]
    debug_meter_reported: AtomicBool,
}

// The pointer owns a Core Audio session whose callback state uses C atomics.
// Lifecycle setup happens before publication; the move-only reader serializes
// pump-side format refresh and PCM consumption. Probe commands own a separate mutex.
unsafe impl Send for ProcessTap {}
// SAFETY: shared access exposes atomic telemetry, never a PCM read. A non-Clone
// reader token owns the only SPSC consumer and requires mutable access to read.
unsafe impl Sync for ProcessTap {}

impl ProcessTap {
    /// Creates and starts a tap of one exact app.
    pub(super) fn start(
        bundle_id: &str,
        target_device_uid: Option<&str>,
    ) -> Result<Self, CaptureProbeError> {
        let tap = Self::prepare(bundle_id, target_device_uid)?;
        tap.start_native()?;
        Ok(tap)
    }

    /// Creates a stopped tap of one exact app.
    pub(super) fn prepare(
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

    pub(super) fn prepare_selector(
        selector: &ApplicationSelector,
        target_device_uid: Option<&str>,
    ) -> Result<Self, CaptureProbeError> {
        let invalid = |operation| {
            CaptureProbeError::new(
                operation,
                TapError {
                    code: NATIVE_INVALID_ARGUMENT,
                    os_status: 0,
                },
            )
        };
        let source_id = mixrelay_macos_transport::capture_selection::source_id(selector)
            .ok_or_else(|| invalid(CaptureOperation::Validate))?;
        let native = TapTarget::new(selector, target_device_uid)
            .ok_or_else(|| invalid(CaptureOperation::Validate))?
            .create()?;
        let stream = stream_spec(native.stats().channels)
            .ok_or_else(|| invalid(CaptureOperation::Prepare))?;
        Ok(Self {
            native,
            reader_claim: AtomicBool::new(false),
            resolved: ResolvedApplicationCapture { source_id, stream },
            #[cfg(debug_assertions)]
            debug_meter_reported: AtomicBool::new(false),
        })
    }

    pub(super) fn start_native(&self) -> Result<(), CaptureProbeError> {
        // SAFETY: caller exclusively owns native control, before any pump handoff.
        checked(CaptureOperation::Start, unsafe {
            mr_process_tap_start(self.native.as_ptr())
        })
    }

    pub(super) fn stop_native(&self) -> Result<(), CaptureProbeError> {
        // SAFETY: the caller is the sole control owner; no ring reader uses a stopped tap.
        checked(CaptureOperation::Stop, unsafe {
            mr_process_tap_stop(self.native.as_ptr())
        })
    }

    /// Applies a pending native format change; the raw error lets callers tell a busy
    /// native side from a failure.
    pub(super) fn refresh_native(&self) -> Result<(), TapError> {
        // SAFETY: the caller is the sole format-control owner of this tap.
        let error = unsafe { mr_process_tap_refresh_format(self.native.as_ptr()) };
        if error.code == NATIVE_OK {
            Ok(())
        } else {
            Err(error)
        }
    }

    pub(super) fn stats(&self) -> TapStats {
        self.native.stats()
    }

    /// Signaled after each tap callback's ring write; `None` means poll on a timer.
    fn wakeup(&self) -> Option<&Notifier> {
        // SAFETY: the notifier lives as long as this session's native owner, which this
        // borrow keeps alive.
        unsafe {
            mr_process_tap_wakeup(self.native.as_ptr())
                .cast::<Notifier>()
                .as_ref()
        }
    }

    fn channels(&self) -> usize {
        usize::from(self.resolved.stream.channels)
    }

    fn take_native_peaks(&self) -> Vec<f32> {
        let mut peaks = [0.0_f32; MAXIMUM_METER_CHANNELS];
        // SAFETY: the destination is writable for the declared capacity and the session is alive.
        let count = unsafe {
            mr_process_tap_take_peaks(
                self.native.as_ptr(),
                peaks.as_mut_ptr(),
                self.channels().min(MAXIMUM_METER_CHANNELS),
            )
        };
        #[cfg(debug_assertions)]
        self.report_first_meter(&peaks[..count]);
        peaks[..count].to_vec()
    }

    #[cfg(debug_assertions)]
    fn report_first_meter(&self, peaks: &[f32]) {
        if !peaks.iter().any(|peak| *peak > 0.0)
            || self
                .debug_meter_reported
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let stats = self.stats();
        eprintln!(
            "application meter started in desktop: running={} callbacks={} captured_frames={} dropped_frames={} available_frames={} channels={} sample_rate={} peaks={peaks:?}",
            stats.running,
            stats.callback_count,
            stats.captured_frames,
            stats.dropped_frames,
            stats.available_frames,
            stats.channels,
            stats.sample_rate,
        );
    }

    /// Claims the tap's only PCM reader; telemetry stays shared through the `Arc`.
    pub(super) fn acquire_reader(self: &Arc<Self>) -> Result<TapReader, String> {
        ExclusiveReader::acquire(Arc::clone(self))
            .map(TapReader)
            .ok_or_else(|| "application capture already has a PCM reader".to_owned())
    }

    /// Reads up to `destination.len() / channels` interleaved frames.
    ///
    /// # Safety
    /// The caller must be the ring's only consumer for the duration of the call.
    unsafe fn read_exclusive(&self, destination: &mut [f32]) -> usize {
        let channels = self.channels();
        if channels == 0 {
            return 0;
        }
        // SAFETY: the caller guarantees exclusive consumption; the destination covers
        // the requested frames.
        unsafe {
            mr_process_tap_read(
                self.native.as_ptr(),
                destination.as_mut_ptr(),
                destination.len() / channels,
            )
        }
    }
}

impl ReaderOwner for ProcessTap {
    fn reader_claim(&self) -> &AtomicBool {
        &self.reader_claim
    }
}

impl ApplicationCaptureSession for ProcessTap {
    fn resolved(&self) -> &ResolvedApplicationCapture {
        &self.resolved
    }

    fn read_interleaved(
        &mut self,
        destination: &mut [f32],
    ) -> Result<usize, HostAudioBackendError> {
        // SAFETY: &mut session excludes shared Arc readers (Arc::get_mut fails
        // while a reader owns an Arc).
        Ok(unsafe { self.read_exclusive(destination) })
    }

    fn take_peaks(&self) -> Result<Vec<f32>, HostAudioBackendError> {
        Ok(self.take_native_peaks())
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

/// The only PCM consumer of a shared tap. Telemetry keeps the `Arc`; only this move-only
/// token reads the ring.
pub(super) struct TapReader(ExclusiveReader<ProcessTap>);

impl TapReader {
    fn tap(&self) -> &ProcessTap {
        self.0.owner()
    }
}

impl CaptureTap for TapReader {
    fn source_id(&self) -> &str {
        &self.tap().resolved.source_id
    }

    fn channels(&self) -> usize {
        self.tap().channels()
    }

    fn health(&self) -> TapStats {
        self.tap().stats()
    }

    fn wait_for_callback(&self, timeout: Duration) -> Option<TapWake> {
        let wakeup = self.tap().wakeup()?;
        let waiting_since = host_time();
        Some(TapWake::after(
            wakeup.wait(timeout),
            waiting_since,
            host_time,
        ))
    }

    fn failed_signals(&self) -> u64 {
        self.tap().wakeup().map_or(0, Notifier::failed_signals)
    }

    fn read_timed(
        &mut self,
        samples: &mut [f32],
        times: &mut [u64],
        generation: TapReadGeneration,
    ) -> usize {
        let tap = self.tap();
        let channels = tap.channels();
        if channels == 0 || samples.len() != times.len() * channels {
            return 0;
        }
        // SAFETY: this move-only reader owns both disjoint outputs and the sole ring cursor.
        unsafe {
            mr_process_tap_read_timed(
                tap.native.as_ptr(),
                samples.as_mut_ptr(),
                times.as_mut_ptr(),
                times.len(),
                generation,
            )
        }
    }

    fn read_interleaved(&mut self, samples: &mut [f32]) -> usize {
        // SAFETY: the atomic lease admits one reader; &mut prevents overlapping
        // calls through it. Its owner Arc outlives the bounded native read.
        unsafe { self.tap().read_exclusive(samples) }
    }

    fn refresh_format(&mut self) -> Result<FormatState, String> {
        // RESOURCE_BUSY retains the native context and requires retry, not destruction
        // or silent raw playback.
        match self.tap().refresh_native() {
            Ok(()) => Ok(FormatState::Ready),
            Err(error) if error.code == NATIVE_RESOURCE_BUSY => Ok(FormatState::Pending),
            Err(error) => Err(format!(
                "application format refresh failed: {} ({})",
                error.code, error.os_status
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_selectors_a_native_tap_can_express_reach_tap_creation() {
        let exact = ApplicationSelector::BundleId {
            bundle_id: "org.example.Player".into(),
            include_descendants: false,
        };
        let target = TapTarget::new(&exact, Some("io.mixrelay.audio.endpoint.0a")).unwrap();
        assert_eq!(target.bundle_id.as_deref(), Some(c"org.example.Player"));
        assert_eq!(
            target.device_uid.as_deref(),
            Some(c"io.mixrelay.audio.endpoint.0a")
        );

        let system = ApplicationSelector::AllExcept {
            bundle_ids: vec!["io.mixrelay.desktop".into(), "org.example.Chat".into()],
        };
        let target = TapTarget::new(&system, None).unwrap();
        assert_eq!(
            target.bundle_id, None,
            "an exclusion list captures system audio"
        );
        assert_eq!(target.excluded_bundle_ids.len(), 2);

        let descendants = ApplicationSelector::BundleId {
            bundle_id: "org.example.Player".into(),
            include_descendants: true,
        };
        let nul = ApplicationSelector::BundleId {
            bundle_id: "org.example\0Player".into(),
            include_descendants: false,
        };
        assert_eq!(TapTarget::new(&descendants, None), None);
        assert_eq!(TapTarget::new(&nul, None), None);
        assert_eq!(TapTarget::new(&exact, Some("bad\0uid")), None);
    }
}
