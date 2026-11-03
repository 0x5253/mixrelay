//! Publishes adapter observations and composes processing, native format and telemetry owners.

mod format;
mod native_format;
mod status;
mod telemetry;

use std::{
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    sync::{Arc, RwLock},
    time::Duration,
};

use mixrelay_audio_graph::RenderFormat;
use mixrelay_audio_io::session::{
    AudioSessionCapabilities, AudioSessionStatus, ClockAuthority, SessionFormat, SessionIssue,
};
use mixrelay_control_protocol::{AppStatus, DriverState, PROTOCOL_VERSION};

use crate::session_settings::{SessionPreferences, StagedSessionPreferences};
use mixrelay_time::{MonotonicClock, Sleeper};

/// Clock and off-thread waiting shared by one processing lifecycle.
pub trait ProcessingTime: MonotonicClock + Sleeper {}
impl<T: MonotonicClock + Sleeper> ProcessingTime for T {}
use crate::{ProcessingControlBackend, RuntimeStatusBackend, SessionFormatBackend};

pub mod processing;
use processing::{ProcessingLease, ProcessingLifecycle, WorkerState};

/// Aggregate capture availability, supplied by native capture owners rather than
/// inferred from a silent meter. No configured application is also `Ready`.
#[derive(Clone, Copy)]
pub enum ApplicationRuntimeState {
    Ready,
    Waiting,
    Ambiguous,
}

/// Supplied by the linked OS implementation, never inferred from its OS name.
/// These are implemented capabilities, not everything the native API could do.
pub struct AdapterProfile {
    pub initial_format: SessionFormat,
    pub capabilities: AudioSessionCapabilities,
    pub application_capture_rates: Vec<u32>,
    pub clock_authority: ClockAuthority,
    pub transport_latency_frames: Option<u32>,
    pub requires_observed_device_rate: bool,
}

/// Reports only I/O that the currently linked adapter can actually execute.
/// This is intentionally not a table of everything the operating system supports.
pub struct AdapterRuntimeStatus {
    processing: ProcessingLifecycle,
    application_state: AtomicU32,
    // Pacing authority, synchronization evidence and explicit inactivity are one
    // publication; readers must never combine fields from different clock owners.
    clock_observation: AtomicU32,
    telemetry: telemetry::ProcessTelemetry,
    profile: AdapterProfile,
    // Control-plane discovery can narrow capabilities without restarting the
    // engine. Never read/write this lock from the real-time processing path.
    discovered_sample_rates: RwLock<Option<Vec<u32>>>,
    endpoint_observation: AtomicU32,
    observed_device_format: AtomicU64,
    // Independent hardware routing authority must not overwrite the own virtual
    // endpoint observation. Bit 63 records selection even while unavailable.
    routing_clock_observation: AtomicU64,
    active_format: AtomicU64,
    failed: AtomicBool,
    native: native_format::NativeFormat,
    preferences: Option<Result<Arc<dyn SessionPreferences>, String>>,
    processing_time: Arc<dyn ProcessingTime>,
    preferred_format: AtomicU64,
    preferences_error: AtomicBool,
    restore_failed: AtomicBool,
}

impl AdapterRuntimeStatus {
    #[must_use]
    pub fn new(profile: AdapterProfile, processing_time: Arc<dyn ProcessingTime>) -> Self {
        let initial_format = profile.initial_format;
        let initial_clock = pack_clock(
            profile.clock_authority,
            profile.clock_authority != ClockAuthority::EngineTimer,
        );
        Self {
            processing: ProcessingLifecycle::default(),
            application_state: AtomicU32::new(0),
            clock_observation: AtomicU32::new(initial_clock),
            telemetry: telemetry::ProcessTelemetry::default(),
            profile,
            discovered_sample_rates: RwLock::new(None),
            endpoint_observation: AtomicU32::new(0),
            observed_device_format: AtomicU64::new(0),
            routing_clock_observation: AtomicU64::new(0),
            active_format: AtomicU64::new(pack_format(RenderFormat {
                sample_rate: initial_format.sample_rate,
                block_frames: initial_format.block_frames,
            })),
            failed: AtomicBool::new(false),
            native: native_format::NativeFormat::default(),
            preferences: None,
            processing_time,
            preferred_format: AtomicU64::new(0),
            preferences_error: AtomicBool::new(false),
            restore_failed: AtomicBool::new(false),
        }
    }

    /// Called by control/monitor threads, never through an OS API in the audio callback.
    pub fn observe_supported_sample_rates(&self, native_rates: &[u32]) {
        let rates = self
            .profile
            .capabilities
            .supported_sample_rates
            .iter()
            .copied()
            .filter(|rate| native_rates.contains(rate))
            .collect();
        *self
            .discovered_sample_rates
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(rates);
    }

    fn supported_sample_rates(&self) -> Vec<u32> {
        self.discovered_sample_rates
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .unwrap_or(&self.profile.capabilities.supported_sample_rates)
            .clone()
    }

    /// Called by control/monitor threads, never through an OS API in the audio callback.
    pub fn observe_device_rate(&self, rate: Option<u32>) {
        self.observe_device_format(rate.map(|sample_rate| RenderFormat {
            sample_rate,
            block_frames: 0,
        }));
    }

    /// Publishes a coherent observed native pair. Zero block frames means a rate-only
    /// adapter; never infer the native buffer size from the DSP processing capacity.
    pub fn observe_device_format(&self, format: Option<RenderFormat>) {
        self.observed_device_format
            .store(format.map_or(0, pack_format), Ordering::Release);
    }

    /// Selects an independent routing clock without relabeling the own HAL rate.
    /// Missing hardware observation is different from returning authority to HAL.
    pub fn observe_routing_clock(&self, selected: bool, sample_rate: Option<u32>) {
        self.routing_clock_observation.store(
            if selected {
                (1_u64 << 63) | u64::from(sample_rate.unwrap_or(0))
            } else {
                0
            },
            Ordering::Release,
        );
    }

    fn routing_clock_rate(&self) -> Option<u32> {
        let value = self.routing_clock_observation.load(Ordering::Acquire);
        (value != 0).then(|| split_words(value).1)
    }

    /// Cached own-endpoint settings for the device UI. Reading these fields does
    /// not query `CoreAudio` and never substitutes the hardware routing rate.
    #[must_use]
    pub fn endpoint_format_settings(&self) -> (Option<u32>, Vec<u32>, bool) {
        let rate = unpack_format(self.observed_device_format.load(Ordering::Acquire)).sample_rate;
        let rates = self.supported_sample_rates();
        let observed = (rate != 0).then_some(rate);
        let writable = observed.is_some()
            && !rates.is_empty()
            && self.profile.capabilities.can_reconfigure_live
            && self.native.has_rate_control()
            && self.native.follows_rate();
        (observed, rates, writable)
    }

    /// Publishes actual native pacing and synchronization evidence together. A device
    /// callback alone is not proof that another independent clock domain is bridged.
    /// This is lock-free and performs no allocation or OS calls on the DSP boundary.
    pub fn observe_clock(&self, authority: ClockAuthority, synchronized: bool) {
        self.clock_observation
            .store(pack_clock(authority, synchronized), Ordering::Release);
    }

    /// The adapter has observed that no clock is currently pacing processing. This is
    /// not inferred from silent PCM. Retain the last authority for diagnostics, but
    /// do not describe inactivity as synchronization evidence or a clock failure.
    /// The next actual `observe_clock` publication leaves this explicit idle state.
    pub fn observe_clock_idle(&self) {
        // The update closure never returns None, so failure is impossible. A single
        // atomic update preserves whichever authority was latest at its commit point.
        let _ = self
            .clock_observation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some((value & CLOCK_AUTHORITY_MASK) | CLOCK_IDLE)
            });
    }

    /// Off-RT capture supervision publishes its current aggregate availability.
    pub fn observe_application_state(&self, state: ApplicationRuntimeState) {
        self.application_state.store(
            match state {
                ApplicationRuntimeState::Ready => 0,
                ApplicationRuntimeState::Waiting => 1,
                ApplicationRuntimeState::Ambiguous => 2,
            },
            Ordering::Release,
        );
    }

    /// Control/monitor path only. Unknown is not Ready, and device existence is
    /// separate from callback activity or a graph attachment.
    pub fn observe_endpoint(&self, present: bool) {
        self.endpoint_observation
            .store(if present { 1 } else { 2 }, Ordering::Release);
    }

    /// Publishes independent Driver service health from a non-audio observer.
    pub fn observe_driver_state(&self, state: mixrelay_driver_control::DriverState) {
        use mixrelay_driver_control::DriverState;
        self.endpoint_observation.store(
            match state {
                DriverState::Ready => 1,
                DriverState::NotInstalled => 2,
                DriverState::InstalledNotLoaded => 0,
                DriverState::Unavailable => 3,
            },
            Ordering::Release,
        );
    }

    /// An attachment no longer supplies current device evidence. Unknown is not
    /// absent: dropping a graph must not claim the installed driver was removed.
    pub fn forget_endpoint_observation(&self) {
        self.endpoint_observation.store(0, Ordering::Release);
        self.observe_device_rate(None);
    }

    /// One processing owner per session. Dropping the guard publishes a stopped
    /// state, including unwinding; callback ownership is never made `Send + Sync`.
    ///
    /// # Errors
    /// Fails if a worker is already running. Restart uses a fresh session object.
    pub fn begin_processing(&self) -> Result<ProcessingLease<'_>, &'static str> {
        self.processing.begin()
    }

    pub fn request_stop(&self) {
        self.processing.request_stop();
    }

    #[must_use]
    pub fn stop_requested(&self) -> bool {
        self.processing.stop_requested()
    }

    /// Lock-free reporting from the DSP boundary. Zero samples are not failures.
    pub fn record_process(&self, report: mixrelay_audio_io::process::ProcessReport) {
        self.telemetry.record(report);
    }

    #[must_use]
    pub fn device_matches(&self, sample_rate: u32) -> bool {
        unpack_format(self.observed_device_format.load(Ordering::Acquire)).sample_rate
            == sample_rate
    }

    #[must_use]
    pub fn with_format_control(mut self, setter: fn(u32) -> Result<(), String>) -> Self {
        self.native.set_rate_control(setter);
        self
    }

    /// Full native pair control. The callback confirms both fields before success;
    /// rollback uses the actual pre-write pair from the observed-format reader.
    #[must_use]
    pub fn with_native_format_control(
        mut self,
        setter: impl Fn(RenderFormat) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.native.set_pair_control(Arc::new(setter));
        self
    }

    /// Follows this adapter's own endpoint, never an unrelated hardware/server clock.
    /// The callback runs only on control threads and may capture owned/weak backend state.
    #[must_use]
    pub fn with_observed_format_following(
        mut self,
        reader: impl Fn() -> Option<RenderFormat> + Send + Sync + 'static,
    ) -> Self {
        self.native.follow_pair(Arc::new(reader));
        self
    }

    /// Explicit engine-format control for adapters whose native streams negotiate their
    /// own formats. Does not imply that the OS device's nominal format can be changed.
    /// The resource backend must stage matching converters and reject fixed-rate devices.
    #[must_use]
    pub fn with_graph_format_control(mut self) -> Self {
        self.native.allow_graph_control();
        self
    }

    /// Opt in only when the adapter can rebuild its processing plan after an external
    /// nominal-rate change. The reader queries native state on the control thread; it
    /// is rechecked after suspension to detect another OS/DAW change during preparation.
    /// For rate-only endpoints, startup follows the current device rate instead of
    /// overwriting another application's setting with a saved last-observed rate.
    #[must_use]
    pub fn with_observed_rate_following(
        mut self,
        reader: impl Fn() -> Option<u32> + Send + Sync + 'static,
    ) -> Self {
        self.native.follow_rate(Arc::new(reader));
        self
    }

    /// Audio worker only: acknowledge suspension at a block boundary without a lock.
    #[must_use]
    pub fn checkpoint(&self) -> bool {
        self.processing.checkpoint()
    }

    /// Inspect without acknowledging: a callback owner must drain its in-flight
    /// render before the supervisor may acknowledge a format transaction.
    #[must_use]
    pub fn pause_requested(&self) -> bool {
        self.processing.pause_requested()
    }

    /// The outer graph/native transaction could not restore its original state.
    /// Keep the existing suspension latched; a successful inner rollback alone
    /// is not proof that the old DSP plan can safely process the current device.
    pub fn fail_reconfiguration(&self) {
        self.failed.store(true, Ordering::Release);
    }

    /// Publish an actually activated plan, never a merely requested format.
    pub fn activate_format(&self, format: RenderFormat) {
        self.active_format
            .store(pack_format(format), Ordering::Release);
    }

    /// Requests and waits for callback-boundary suspension on the control thread.
    /// # Errors
    /// Returns an error if the running audio owner does not acknowledge in time.
    pub fn pause_at_boundary(&self) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        // Fresh tickets prevent a previous suspension acknowledgement from authorizing
        // native changes while the worker has already resumed.
        let ticket = self.processing.request_pause();
        if !self.processing.await_pause(
            ticket,
            Duration::from_secs(1),
            self.processing_time.as_ref(),
        ) {
            self.resume();
            return Err(mixrelay_audio_io::HostAudioBackendError::BackendFailure {
                message: "audio worker did not acknowledge suspension".into(),
            });
        }
        Ok(())
    }
}

const CLOCK_AUTHORITY_MASK: u32 = 3;
const CLOCK_SYNCHRONIZED: u32 = 1 << 2;
const CLOCK_IDLE: u32 = 1 << 3;

const fn pack_clock(authority: ClockAuthority, synchronized: bool) -> u32 {
    let authority = match authority {
        ClockAuthority::EngineTimer => 0,
        ClockAuthority::Device => 1,
        ClockAuthority::AudioServer => 2,
        ClockAuthority::Host => 3,
    };
    authority | if synchronized { CLOCK_SYNCHRONIZED } else { 0 }
}

struct ClockObservation {
    authority: ClockAuthority,
    synchronized: bool,
    idle: bool,
}

impl ClockObservation {
    const fn unpack(value: u32) -> Self {
        Self {
            authority: match value & CLOCK_AUTHORITY_MASK {
                1 => ClockAuthority::Device,
                2 => ClockAuthority::AudioServer,
                3 => ClockAuthority::Host,
                _ => ClockAuthority::EngineTimer,
            },
            synchronized: value & CLOCK_SYNCHRONIZED != 0,
            idle: value & CLOCK_IDLE != 0,
        }
    }
}

const fn pack_format(format: RenderFormat) -> u64 {
    ((format.sample_rate as u64) << 32) | format.block_frames as u64
}

fn unpack_format(value: u64) -> RenderFormat {
    let (sample_rate, block_frames) = split_words(value);
    RenderFormat {
        sample_rate,
        block_frames,
    }
}

/// Splits a packed atomic word into its upper and lower 32-bit halves.
const fn split_words(value: u64) -> (u32, u32) {
    let [h0, h1, h2, h3, l0, l1, l2, l3] = value.to_be_bytes();
    (
        u32::from_be_bytes([h0, h1, h2, h3]),
        u32::from_be_bytes([l0, l1, l2, l3]),
    )
}

impl ProcessingControlBackend for AdapterRuntimeStatus {
    fn suspend(&self) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        self.pause_at_boundary()
    }

    fn fail_reconfiguration(&self) {
        Self::fail_reconfiguration(self);
    }

    fn resume(&self) {
        if !self.failed.load(Ordering::Acquire) {
            self.processing.resume();
        }
    }
}

#[cfg(test)]
#[path = "session_clock_tests.rs"]
mod clock_tests;

#[cfg(feature = "test-util")]
impl AdapterRuntimeStatus {
    pub fn test_set_preferences(
        &mut self,
        preferences: Result<Arc<dyn SessionPreferences>, String>,
    ) {
        self.preferences = Some(preferences);
    }
    pub fn test_restore_saved_format(
        &self,
        requested: RenderFormat,
        fallback: RenderFormat,
    ) -> RenderFormat {
        self.restore_saved_format(requested, fallback)
    }
}

#[cfg(feature = "test-util")]
impl AdapterRuntimeStatus {
    pub fn test_set_preferred_format(&self, format: RenderFormat) {
        self.preferred_format
            .store(pack_format(format), Ordering::Release);
    }
    #[must_use]
    pub fn test_preferred_format(&self) -> Option<SessionFormat> {
        self.preferred_format()
    }
    #[must_use]
    pub fn test_preferences(&self) -> Option<Result<Arc<dyn SessionPreferences>, String>> {
        self.preferences.clone()
    }
    #[must_use]
    pub fn test_restore_failed(&self) -> bool {
        self.restore_failed.load(Ordering::Acquire)
    }
    #[must_use]
    pub fn test_preferences_error(&self) -> bool {
        self.preferences_error.load(Ordering::Acquire)
    }
}

#[cfg(feature = "test-util")]
impl AdapterRuntimeStatus {
    /// # Errors
    /// Reports the original native restoration failure.
    pub fn test_restore_native_format(
        &self,
        requested: RenderFormat,
        fallback: RenderFormat,
    ) -> Result<(), String> {
        self.restore_native_format(requested, fallback)
    }
    #[must_use]
    pub fn test_active_format(&self) -> RenderFormat {
        unpack_format(self.active_format.load(Ordering::Acquire))
    }
    #[must_use]
    pub fn test_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_io::session::AudioBackendKind;
    pub(super) fn profile(backend: AudioBackendKind) -> AdapterProfile {
        AdapterProfile {
            initial_format: SessionFormat {
                sample_rate: 44_100,
                block_frames: 128,
            },
            capabilities: AudioSessionCapabilities {
                backend,
                supported_sample_rates: vec![44_100, 48_000, 96_000],
                supported_block_frames: vec![64, 128, 256],
                can_reconfigure_live: false,
                hardware_input: true,
                hardware_output: false,
                virtual_endpoint: true,
                application_capture: true,
                sample_rate_conversion: false,
                drift_correction: false,
            },
            application_capture_rates: vec![44_100, 96_000],
            clock_authority: ClockAuthority::AudioServer,
            transport_latency_frames: None,
            requires_observed_device_rate: false,
        }
    }
}
