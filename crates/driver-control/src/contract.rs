use mixrelay_audio_io::{
    NativeSampleFormat, NativeStreamFormat, VirtualEndpointId, VirtualEndpointSettings,
    VirtualEndpointSpec,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Driver snapshot GET route.
pub const SNAPSHOT_ROUTE: &str = "/api/v1/driver";
/// Driver command PUT route.
pub const COMMAND_ROUTE: &str = SNAPSHOT_ROUTE;
/// Maximum serialized Driver command, snapshot and settings size.
pub const JSON_LIMIT: usize = 1024 * 1024;

/// Native installation/load state, independent of graph engine activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverState {
    /// Installed native backend is available.
    Ready,
    /// Native Driver is absent.
    NotInstalled,
    /// Native Driver is installed but not loaded.
    InstalledNotLoaded,
    /// Native discovery or ownership is unavailable.
    Unavailable,
}

/// OS playback into the device versus OS capture from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDirection {
    /// OS playback.
    Render,
    /// OS capture.
    Capture,
}

/// Persisted native device intent; format fields retain directional PCM representations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverDeviceConfig {
    /// Stable identity, name, geometry and shared nominal sample rate.
    pub spec: VirtualEndpointSpec,
    /// Desired render format, or no explicit format request.
    pub render_format: Option<NativeStreamFormat>,
    /// Desired capture format, or no explicit format request.
    pub capture_format: Option<NativeStreamFormat>,
}

/// Actual native observation and persistence status at a monotonic control revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverSnapshot {
    /// Wire protocol version, currently 1.
    pub protocol_version: u32,
    /// Revision required by the next mutation.
    pub revision: u64,
    /// Native installation/load state.
    pub state: DriverState,
    /// Installed service release version supplied by the binary.
    pub installed_version: Option<String>,
    /// Actual Driver service process owning native devices; absent for unavailable UI snapshots.
    #[serde(default)]
    pub owner_process_id: Option<u32>,
    /// Actual OS capabilities and device readback; never fabricated from persisted intent.
    pub settings: VirtualEndpointSettings,
    /// Save failure after a successful native change; active state is not rolled back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_error: Option<String>,
}

/// Revision-checked commands serialized by the independent Driver owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DriverCommand {
    /// Restore retained devices after an explicit Driver manager quit.
    Activate { expected_revision: u64 },
    /// Stop native devices and audio return path without deleting saved device settings.
    Deactivate { expected_revision: u64 },
    /// Create a previously absent identity.
    Create {
        expected_revision: u64,
        spec: VirtualEndpointSpec,
    },
    /// Configure an existing identity.
    Configure {
        expected_revision: u64,
        spec: VirtualEndpointSpec,
    },
    /// Remove an existing identity without reviving it later.
    Remove {
        expected_revision: u64,
        endpoint_id: VirtualEndpointId,
    },
    /// Select one exact supported native directional tuple.
    SetFormat {
        expected_revision: u64,
        endpoint_id: VirtualEndpointId,
        direction: NativeDirection,
        format: NativeStreamFormat,
    },
    /// Select one PCM representation for every present direction in one native change.
    /// Each direction retains its actual sample rate and channel count; all resulting tuples
    /// must be supported, and every present direction must have an observed current format.
    SetSampleFormat {
        expected_revision: u64,
        endpoint_id: VirtualEndpointId,
        sample_format: NativeSampleFormat,
    },
}

impl DriverCommand {
    pub(crate) const fn expected_revision(&self) -> u64 {
        match self {
            Self::Activate { expected_revision }
            | Self::Deactivate { expected_revision }
            | Self::Create {
                expected_revision, ..
            }
            | Self::Configure {
                expected_revision, ..
            }
            | Self::Remove {
                expected_revision, ..
            }
            | Self::SetFormat {
                expected_revision, ..
            }
            | Self::SetSampleFormat {
                expected_revision, ..
            } => *expected_revision,
        }
    }
}

/// Native owner boundary. Implementations prepare/drain/apply/read back and restore on failure.
pub trait DriverBackend: Send {
    /// Reads actual native settings. Returns discovery or backend errors without invented formats.
    /// # Errors
    /// Returns an error when actual state cannot be observed.
    fn settings(&self) -> Result<VirtualEndpointSettings, DriverError>;
    /// Replaces the complete inventory. An error must preserve the previous active inventory.
    /// # Errors
    /// Returns validation, negotiation, or native preparation/rollback errors.
    fn replace(&mut self, desired: &[DriverDeviceConfig]) -> Result<(), DriverError>;
    /// Restores failed native owners from retained `desired` inventory.
    /// Returns true after recovery; healthy or merely paused owners remain unchanged.
    /// Backends with an independent OS-owned lifetime do not need recovery here.
    /// # Errors
    /// Returns native preparation, negotiation or rollback errors without changing intent.
    fn recover(&mut self, _desired: &[DriverDeviceConfig]) -> Result<bool, DriverError> {
        Ok(false)
    }
    /// Publishes ephemeral input latency on control. Old/non-HAL backends safely ignore it.
    /// # Errors
    /// Returns native publication errors without changing persisted settings.
    fn publish_latency(&mut self, _publication: &crate::RuntimeLatency) -> Result<(), DriverError> {
        Ok(())
    }
    /// Reports native installation/load state independently of service reachability.
    fn state(&self) -> DriverState;
}

/// The transport's own error, kept as the source so logs can show the OS cause.
pub type TransportFailure = Box<dyn std::error::Error + Send + Sync>;

/// Control, native, persistence and transport failures kept distinct for callers.
#[derive(Debug, Error)]
pub enum DriverError {
    /// Invalid geometry, capability or command.
    #[error("invalid Driver request: {0}")]
    Invalid(String),
    /// Caller observed an older revision.
    #[error("Driver revision mismatch: expected {expected}, current {current}")]
    RevisionMismatch { expected: u64, current: u64 },
    /// Identity no longer exists.
    #[error("Driver endpoint not found: {0:?}")]
    NotFound(VirtualEndpointId),
    /// Native owner is not ready.
    #[error("Driver unavailable: {0}")]
    Unavailable(String),
    /// Native adapter failure.
    #[error("Driver backend: {0}")]
    Backend(String),
    /// The HAL is still performing an earlier configuration step. The previous
    /// inventory remains current and is republished for the HAL to converge to.
    #[error("Driver configuration is still pending in the HAL; retry after it settles")]
    ConfigurationPending,
    /// Filesystem failure.
    #[error("Driver settings I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid JSON.
    #[error("Driver JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The Driver service could not be reached, so the request was not delivered.
    #[error("Driver service is not reachable")]
    NotReachable(#[source] TransportFailure),
    /// The request failed in transport and may or may not have been delivered, or the
    /// client could not be built and nothing was sent.
    #[error("Driver request failed in transport")]
    Transport(#[source] TransportFailure),
    /// Service rejected a request.
    #[error("Driver service returned {status}: {message}")]
    Remote { status: u16, message: String },
}

#[cfg(test)]
mod tests;
