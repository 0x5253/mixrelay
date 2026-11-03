//! Runtime audio contracts. OS names do not imply a driver, clock, or conversion capability.
//!
//! A sample rate describes frames per second; a block size describes scheduling. Neither
//! proves that two streams share a clock. Backends must report the three independently.

use serde::{Deserialize, Serialize};

/// The existing timer-driven adapters' format, not a product-wide supported-rate limit.
pub const CURRENT_TIMER_FORMAT: SessionFormat = SessionFormat {
    sample_rate: 48_000,
    block_frames: 128,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFormat {
    pub sample_rate: u32,
    pub block_frames: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReconfigureRequest {
    pub expected_revision: u64,
    pub format: SessionFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBackendKind {
    CoreAudio,
    PipeWire,
    ContractOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockAuthority {
    EngineTimer,
    Device,
    AudioServer,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent backend capabilities, not mutually exclusive runtime states"
)]
pub struct AudioSessionCapabilities {
    pub backend: AudioBackendKind,
    pub supported_sample_rates: Vec<u32>,
    pub supported_block_frames: Vec<u32>,
    pub can_reconfigure_live: bool,
    pub hardware_input: bool,
    pub hardware_output: bool,
    pub virtual_endpoint: bool,
    pub application_capture: bool,
    pub sample_rate_conversion: bool,
    pub drift_correction: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionIssue {
    DeviceUnavailable,
    /// Processing owner exited unexpectedly; this does not imply driver removal.
    ProcessingFailed,
    ApplicationUnavailable,
    ApplicationAmbiguous,
    SampleRateMismatch,
    BlockSizeMismatch,
    ClockNotSynchronized,
    InvalidFormat,
    ReconfigurationFailed,
    PreferencesUnavailable,
    StoredFormatUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSessionStatus {
    /// Saving device settings can fail independently from the native audio clock.
    #[serde(default)]
    pub endpoint_preferences_unavailable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_endpoint_settings: Option<crate::VirtualEndpointSettings>,
    #[serde(default)]
    pub processing_state: crate::process::ProcessingState,
    /// Explicit discontinuities reported at native I/O boundaries, not silence.
    #[serde(default)]
    pub discontinuities: u64,
    pub format: SessionFormat,
    /// Last stored preference; it can differ from the active format after failed restoration.
    #[serde(default)]
    pub preferred_format: Option<SessionFormat>,
    pub clock_authority: ClockAuthority,
    /// None means unobserved, not that the device runs at the engine rate.
    pub observed_device_sample_rate: Option<u32>,
    /// Actual native buffer size, independently observed; absent for rate-only adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_device_buffer_frames: Option<u32>,
    pub issue: Option<SessionIssue>,
    /// Known transport delay only. Never label this as measured round-trip latency.
    pub transport_latency_frames: Option<u32>,
    pub capabilities: AudioSessionCapabilities,
}
