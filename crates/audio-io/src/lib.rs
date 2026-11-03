use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod buffer_policy;
pub mod capture_alignment;
pub mod capture_read;
pub mod client_pcm;
pub mod clock_bridge;
mod device;
pub mod discovery;
pub mod format_contract;
pub mod formats;
pub mod frame_budget;
mod native_format;
pub mod pcm;
pub mod pcm_buffer;
pub mod pcm_fifo;
pub mod process;
pub mod recovery;
pub mod residence;
pub mod retention;
pub use device::{DeviceDirection, DeviceSelector};
pub use native_format::{
    NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat, NativeStreamFormats,
};
pub mod session;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VirtualEndpointId(String);

impl VirtualEndpointId {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for VirtualEndpointId {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLayout {
    Mono,
    Stereo,
    Discrete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSpec {
    pub channels: u16,
    pub channel_layout: ChannelLayout,
}

impl StreamSpec {
    fn validate(self, maximum: u16) -> Result<(), HostAudioContractError> {
        if self.channels == 0 || self.channels > maximum {
            return Err(HostAudioContractError::InvalidChannelCount {
                maximum,
                found: self.channels,
            });
        }
        let expected = match self.channel_layout {
            ChannelLayout::Mono => Some(1),
            ChannelLayout::Stereo => Some(2),
            ChannelLayout::Discrete => None,
        };
        if let Some(expected) = expected
            && self.channels != expected
        {
            return Err(HostAudioContractError::ChannelLayoutMismatch {
                layout: self.channel_layout,
                expected,
                found: self.channels,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualEndpointSpec {
    pub id: VirtualEndpointId,
    /// Nonempty native device name without an interior NUL byte.
    pub display_name: String,
    /// OS playback/output stream. Applications and the engine write PCM to this stream.
    pub render: Option<StreamSpec>,
    /// OS recording/input stream. Applications and the engine read PCM from this stream.
    pub capture: Option<StreamSpec>,
    pub supported_sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
}

impl VirtualEndpointSpec {
    /// Heap capacity owned by this specification, excluding the inline struct.
    /// Runtime-only graph nodes use this without serializing native configuration.
    #[must_use]
    pub fn owned_heap_capacity_bytes(&self) -> usize {
        self.id
            .0
            .capacity()
            .saturating_add(self.display_name.capacity())
            .saturating_add(
                self.supported_sample_rates
                    .capacity()
                    .saturating_mul(size_of::<u32>()),
            )
    }

    /// Validates the platform-independent endpoint contract.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioContractError`] when the identity, display name, streams, or sample rates
    /// are invalid.
    pub fn validate_contract(
        &self,
        max_render_channels: u16,
        max_capture_channels: u16,
    ) -> Result<(), HostAudioContractError> {
        validate_name("virtual endpoint ID", self.id.as_str())?;
        validate_name("virtual endpoint display name", &self.display_name)?;
        if self.render.is_none() && self.capture.is_none() {
            return Err(HostAudioContractError::EndpointHasNoStreams);
        }
        if let Some(render) = self.render {
            render.validate(max_render_channels)?;
        }
        if let Some(capture) = self.capture {
            capture.validate(max_capture_channels)?;
        }
        self.validate_requested_sample_rates()
    }

    /// Validates a desired endpoint against one operating-system backend.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioContractError`] when the endpoint cannot be represented safely.
    pub fn validate(
        &self,
        capabilities: &VirtualEndpointCapabilities,
    ) -> Result<(), HostAudioContractError> {
        self.validate_contract(
            capabilities.max_render_channels,
            capabilities.max_capture_channels,
        )?;
        if self.render.is_some() && self.capture.is_some() && !capabilities.supports_duplex {
            return Err(HostAudioContractError::DuplexUnsupported);
        }
        self.validate_available_sample_rates(capabilities)
    }

    fn validate_requested_sample_rates(&self) -> Result<(), HostAudioContractError> {
        let requested: BTreeSet<_> = self.supported_sample_rates.iter().copied().collect();
        if requested.len() != self.supported_sample_rates.len()
            || requested.contains(&0)
            || !requested.contains(&self.default_sample_rate)
        {
            return Err(HostAudioContractError::InvalidSampleRates);
        }
        Ok(())
    }

    fn validate_available_sample_rates(
        &self,
        capabilities: &VirtualEndpointCapabilities,
    ) -> Result<(), HostAudioContractError> {
        let requested: BTreeSet<_> = self.supported_sample_rates.iter().copied().collect();
        let available: BTreeSet<_> = capabilities
            .supported_sample_rates
            .iter()
            .copied()
            .collect();
        if !requested.is_subset(&available) {
            return Err(HostAudioContractError::UnsupportedSampleRate);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualEndpointCapabilities {
    pub max_render_channels: u16,
    pub max_capture_channels: u16,
    pub supported_sample_rates: Vec<u32>,
    pub supports_duplex: bool,
    pub supports_dynamic_creation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum ApplicationSelector {
    BundleId {
        bundle_id: String,
        #[serde(default = "default_true")]
        include_descendants: bool,
    },
    ExecutablePath {
        path: String,
    },
    ProcessId {
        process_id: u32,
        #[serde(default = "default_true")]
        include_descendants: bool,
    },
    AllExcept {
        bundle_ids: Vec<String>,
    },
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePlaybackPolicy {
    Preserve,
    MuteWhileCaptured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationCaptureRequest {
    pub selector: ApplicationSelector,
    pub stream: StreamSpec,
    pub source_playback: SourcePlaybackPolicy,
    /// User-selected endpoint intent; the native adapter must verify ownership and route binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarding_endpoint: Option<VirtualEndpointId>,
    #[serde(default)]
    pub gain_db: f32,
    /// A restored PID has no durable process identity. Keep its graph node silent
    /// until the user explicitly selects an application again.
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_reselection: bool,
}

// Serde's skip predicate receives a reference to the serialized field.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

impl ApplicationCaptureRequest {
    /// Validates the platform-independent application capture contract.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioContractError`] when the selector, stream, or forwarding intent is invalid.
    pub fn validate_contract(&self, max_channels: u16) -> Result<(), HostAudioContractError> {
        validate_selector(&self.selector)?;
        self.stream.validate(max_channels)?;
        if !self.gain_db.is_finite() {
            return Err(HostAudioContractError::InvalidGain);
        }
        match (self.source_playback, &self.forwarding_endpoint) {
            (SourcePlaybackPolicy::Preserve, None) => {}
            (SourcePlaybackPolicy::Preserve, Some(_)) => {
                return Err(HostAudioContractError::UnexpectedForwardingEndpoint);
            }
            (SourcePlaybackPolicy::MuteWhileCaptured, None) => {
                return Err(HostAudioContractError::ForwardingEndpointRequired);
            }
            (SourcePlaybackPolicy::MuteWhileCaptured, Some(endpoint)) => {
                validate_name("forwarding endpoint ID", endpoint.as_str())?;
                if self.requires_reselection
                    || matches!(&self.selector, ApplicationSelector::AllExcept { .. })
                {
                    return Err(HostAudioContractError::ForwardingRequiresExactSelection);
                }
            }
        }
        Ok(())
    }

    /// Validates a capture request against one operating-system backend.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioContractError`] when the selector or stream is unsupported.
    pub fn validate(
        &self,
        capabilities: &ApplicationCaptureCapabilities,
    ) -> Result<(), HostAudioContractError> {
        self.validate_contract(capabilities.max_channels)?;
        let selector_kind = ApplicationSelectorKind::from(&self.selector);
        if !capabilities.supported_selectors.contains(&selector_kind) {
            return Err(HostAudioContractError::ApplicationSelectorUnsupported);
        }
        if self.source_playback == SourcePlaybackPolicy::MuteWhileCaptured
            && !capabilities.supports_mute_while_captured
        {
            return Err(HostAudioContractError::MuteWhileCapturedUnsupported);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApplicationSelectorKind {
    BundleId,
    ExecutablePath,
    ProcessId,
    AllExcept,
}

impl From<&ApplicationSelector> for ApplicationSelectorKind {
    fn from(selector: &ApplicationSelector) -> Self {
        match selector {
            ApplicationSelector::BundleId { .. } => Self::BundleId,
            ApplicationSelector::ExecutablePath { .. } => Self::ExecutablePath,
            ApplicationSelector::ProcessId { .. } => Self::ProcessId,
            ApplicationSelector::AllExcept { .. } => Self::AllExcept,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationCaptureCapabilities {
    pub max_channels: u16,
    pub supported_selectors: BTreeSet<ApplicationSelectorKind>,
    pub supports_mute_while_captured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedApplicationCapture {
    pub source_id: String,
    pub stream: StreamSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturableApplication {
    pub bundle_id: String,
    pub display_name: String,
    pub icon_data_url: Option<String>,
    pub process_id: u32,
    pub selector: ApplicationSelector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceDescriptor {
    pub uid: String,
    pub display_name: String,
    pub input_channels: u32,
    pub output_channels: u32,
    pub is_default_input: bool,
    pub is_default_output: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicationCaptureHealth {
    pub callback_count: u64,
    pub captured_frames: u64,
    pub dropped_frames: u64,
    pub available_frames: usize,
    pub running: bool,
}

/// Control-plane policy, distinct from low-level transport capacity. A backend may
/// transport many channels without offering device creation or reconfiguration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualEndpointSettings {
    pub can_create: bool,
    pub can_rename: bool,
    pub render_channels: Vec<u16>,
    pub capture_channels: Vec<u16>,
    /// Capabilities for a new identity; never an implicitly retained system device.
    pub creation_template: Option<VirtualEndpointCreationTemplate>,
    /// Referenced devices, keyed by their stable system identity rather than graph node ID.
    pub devices: Vec<VirtualEndpointDeviceSettings>,
}

/// Initial format choices for creating an independently named system device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualEndpointCreationTemplate {
    pub render: Option<StreamSpec>,
    pub capture: Option<StreamSpec>,
    pub supported_sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
}

/// One device's shared configuration and actual native format-control capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualEndpointDeviceSettings {
    pub spec: VirtualEndpointSpec,
    /// Exact OS discovery identifiers for this device; never synthesized by the UI.
    pub native_device_ids: Vec<String>,
    pub observed_sample_rate: Option<u32>,
    pub supported_sample_rates: Vec<u32>,
    pub can_set_sample_rate: bool,
    /// Native directional format tuples, excluding client-side OS conversion.
    pub native_formats: NativeDeviceFormats,
}

impl VirtualEndpointSettings {
    #[must_use]
    pub fn configurable(capabilities: &VirtualEndpointCapabilities) -> Self {
        Self {
            can_create: true,
            can_rename: true,
            render_channels: (1..=capabilities.max_render_channels).collect(),
            capture_channels: (1..=capabilities.max_capture_channels).collect(),
            creation_template: Some(VirtualEndpointCreationTemplate {
                render: (capabilities.max_render_channels > 0).then_some(StreamSpec {
                    channels: capabilities.max_render_channels.min(2),
                    channel_layout: ChannelLayout::Discrete,
                }),
                capture: (capabilities.max_capture_channels > 0).then_some(StreamSpec {
                    channels: capabilities.max_capture_channels.min(2),
                    channel_layout: ChannelLayout::Discrete,
                }),
                supported_sample_rates: formats::SAMPLE_RATES.to_vec(),
                default_sample_rate: 48_000,
            }),
            devices: Vec::new(),
        }
    }
}

pub trait VirtualEndpointBackend: Send + Sync {
    fn capabilities(&self) -> &VirtualEndpointCapabilities;

    /// Reads the actual nominal rate of the identified system device on control.
    /// # Errors
    /// Rejects unavailable devices and backends without device format control.
    fn sample_rate(&self, _id: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "virtual device nominal-rate observation".into(),
        })
    }

    /// Sets one system device's nominal rate; never targets an implicit default.
    /// The controller owns suspension, readback, graph publication and rollback.
    /// # Errors
    /// Rejects unavailable devices, unsupported rates and native write failures.
    fn set_sample_rate(
        &self,
        _id: &VirtualEndpointId,
        _sample_rate: u32,
    ) -> Result<(), HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "virtual device nominal-rate control".into(),
        })
    }

    /// An unavailable control surface is explicit. Native adapters declare
    /// settings without inferring support from transport maxima or OS names.
    fn settings(&self) -> Option<VirtualEndpointSettings> {
        None
    }

    /// Validates/stages graph attachments to OS endpoints on the control thread.
    /// This is NOT driver installation or OS-device deletion. An installed HAL
    /// device outlives every graph; graph clients never create or delete it. Native
    /// resource activation/retirement belongs to the graph-resource transaction.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioBackendError`] when attachments cannot be prepared.
    fn prepare_graph_attachments(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError>;

    /// Resolves graph references to authoritative Driver specifications on control.
    /// Production adapters return current Driver settings and reject absent devices;
    /// copied graph metadata is never a device creation/configuration request.
    /// # Errors
    /// Returns an attachment error when an identity is missing or unavailable.
    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        Ok(desired.to_vec())
    }

    /// Discards this transaction's unpublished attachments when subsequent native
    /// resource preparation fails. Called on the serialized control thread only,
    /// before its processor is published; it must not affect the active graph.
    fn cancel_prepared_graph_attachments(&self) {}
}

pub trait ApplicationCaptureBackend: Send + Sync {
    type Session: ApplicationCaptureSession;

    fn capabilities(&self) -> &ApplicationCaptureCapabilities;

    /// Opens an owned application capture session.
    /// Dropping the returned session must stop callbacks and release every native resource.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioBackendError`] when capture is unavailable or permission is denied.
    fn open(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<Self::Session, HostAudioBackendError>;
}

pub trait ApplicationCaptureSession: Send + Sync {
    fn resolved(&self) -> &ResolvedApplicationCapture;

    /// Pulls interleaved PCM frames from the platform-owned bounded transport.
    /// The destination length and resolved channel count determine the maximum frame count.
    /// Exclusive access is required: a shared telemetry handle must not create
    /// a second consumer of the same single-consumer audio ring.
    ///
    /// ```compile_fail,E0596
    /// use mixrelay_audio_io::ApplicationCaptureSession;
    /// fn read_shared(session: &dyn ApplicationCaptureSession, pcm: &mut [f32]) {
    ///     let _ = session.read_interleaved(pcm);
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioBackendError`] when the native stream is no longer readable.
    fn read_interleaved(
        &mut self,
        _destination: &mut [f32],
    ) -> Result<usize, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "interleaved application audio transport".to_owned(),
        })
    }

    /// Atomically consumes the per-channel peaks accumulated since the previous read.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioBackendError`] when native metering is unavailable.
    fn take_peaks(&self) -> Result<Vec<f32>, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "application audio peak metering".to_owned(),
        })
    }

    /// Reports callback and bounded-transport health independently from signal silence.
    ///
    /// # Errors
    ///
    /// Returns [`HostAudioBackendError`] when the native stream cannot report health.
    fn health(&self) -> Result<ApplicationCaptureHealth, HostAudioBackendError> {
        Err(HostAudioBackendError::CapabilityUnavailable {
            capability: "application audio stream health".to_owned(),
        })
    }
}

pub struct HostAudioServices<V, A> {
    virtual_endpoints: V,
    application_capture: A,
}

impl<V, A> HostAudioServices<V, A> {
    #[must_use]
    pub const fn new(virtual_endpoints: V, application_capture: A) -> Self {
        Self {
            virtual_endpoints,
            application_capture,
        }
    }

    #[must_use]
    pub const fn virtual_endpoints(&self) -> &V {
        &self.virtual_endpoints
    }

    #[must_use]
    pub const fn application_capture(&self) -> &A {
        &self.application_capture
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HostAudioContractError {
    #[error("{field} cannot be empty")]
    EmptyValue { field: &'static str },
    /// Rejects native-bound text before C-string conversion or OS mutation.
    #[error("{field} cannot contain a NUL byte")]
    InteriorNul { field: &'static str },
    #[error("a virtual endpoint requires at least one render or capture stream")]
    EndpointHasNoStreams,
    #[error("the platform backend does not support duplex virtual endpoints")]
    DuplexUnsupported,
    #[error("channel count must be between 1 and {maximum}, found {found}")]
    InvalidChannelCount { maximum: u16, found: u16 },
    #[error("{layout:?} requires {expected} channel(s), found {found}")]
    ChannelLayoutMismatch {
        layout: ChannelLayout,
        expected: u16,
        found: u16,
    },
    #[error("sample rates must be nonzero, unique, and contain the default")]
    InvalidSampleRates,
    #[error("gain must be finite")]
    InvalidGain,
    #[error("the platform backend does not support every requested sample rate")]
    UnsupportedSampleRate,
    #[error("the platform backend does not support this application selector")]
    ApplicationSelectorUnsupported,
    #[error("preserved application playback cannot name a forwarding endpoint")]
    UnexpectedForwardingEndpoint,
    #[error("muted application playback requires a MixRelay forwarding endpoint")]
    ForwardingEndpointRequired,
    #[error("application forwarding requires an explicitly selected exact source")]
    ForwardingRequiresExactSelection,
    #[error("the platform backend cannot mute an application while capturing")]
    MuteWhileCapturedUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HostAudioBackendError {
    #[error("host audio permission decision pending: {capability}")]
    PermissionPending { capability: String },
    #[error("host audio permission denied: {capability}")]
    PermissionDenied { capability: String },
    #[error("host audio capability unavailable: {capability}")]
    CapabilityUnavailable { capability: String },
    #[error("host audio backend failure: {message}")]
    BackendFailure { message: String },
}

fn validate_name(field: &'static str, value: &str) -> Result<(), HostAudioContractError> {
    if value.trim().is_empty() {
        return Err(HostAudioContractError::EmptyValue { field });
    }
    if value.contains('\0') {
        return Err(HostAudioContractError::InteriorNul { field });
    }
    Ok(())
}

fn validate_selector(selector: &ApplicationSelector) -> Result<(), HostAudioContractError> {
    match selector {
        ApplicationSelector::BundleId { bundle_id, .. } => {
            validate_name("application bundle ID", bundle_id)
        }
        ApplicationSelector::ExecutablePath { path } => {
            validate_name("application executable path", path)
        }
        ApplicationSelector::ProcessId { process_id, .. } => {
            if *process_id == 0 {
                return Err(HostAudioContractError::EmptyValue {
                    field: "application process ID",
                });
            }
            Ok(())
        }
        ApplicationSelector::AllExcept { bundle_ids } => bundle_ids
            .iter()
            .try_for_each(|bundle_id| validate_name("excluded application bundle ID", bundle_id)),
    }
}

#[cfg(test)]
mod tests;

pub mod recording;
