//! Platform-independent messages exchanged by control clients and the engine.
use serde::{Deserialize, Serialize};
mod control_failure;
pub use control_failure::{
    ControlBoundary, ControlFailureCode, ControlFailureContext, ControlPlatform, ControlStage,
    FailureDelivery,
};
#[cfg(test)]
mod reply_fixtures;
pub mod runtime_link;
mod telemetry;
mod telemetry_window;
pub use telemetry::{
    ControlStateSnapshot, ControlTelemetry, TelemetryDecodeError, TelemetryDecoder,
    TelemetryEncoder,
};
mod editor_surface;
mod node_control;
mod workspace;
pub use editor_surface::{EditorPointerKind, ProcessorEditorFrame, ProcessorEditorInput};
pub use node_control::{
    EngineControlSnapshot, GraphControlSnapshot, GraphNodeControls, NodeControlCapabilities,
    NodeControlDescriptor, NodeControlPreview, NodeControlRequest, NodeControlResponse,
    NodeControlSnapshot, ObservedRecording, RecorderControlRequest,
};
pub use workspace::{WorkspaceAction, WorkspacePreview, WorkspaceRequest};

/// Engine-authored capture ownership, independent of the graph visible in an editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationCaptureManifest {
    pub protocol_version: u16,
    pub revision: u64,
    pub sources: Vec<ApplicationCaptureSource>,
}

/// One canonical capture owner and the exact request its native adapter must fulfill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationCaptureSource {
    pub node_id: String,
    pub request: mixrelay_audio_io::ApplicationCaptureRequest,
}
pub use mixrelay_audio_io::session::SessionReconfigureRequest;

/// Changes one identified virtual device, not the processing clock of unrelated routes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VirtualEndpointFormatRequest {
    pub expected_revision: u64,
    pub endpoint_id: mixrelay_audio_io::VirtualEndpointId,
    pub sample_rate: u32,
}

pub const PROTOCOL_VERSION: u16 = 1;

/// Publishes graph topology while retaining surviving providers' native state.
/// Only explicit, bounded control edits overlay that state during publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphApplyRequest {
    #[serde(flatten)]
    pub document: mixrelay_audio_graph::GraphDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processor_parameter_changes: Vec<GraphProcessorParameterChange>,
    /// The graph this document was authored for. When present, the engine rejects the
    /// request as stale unless that graph is still selected, so a selection change by
    /// another client cannot redirect the document onto a different graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_graph_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphProcessorParameterChange {
    pub node_id: String,
    pub type_id: String,
    pub parameter_id: u32,
    pub value: f64,
}

impl From<mixrelay_audio_graph::GraphDocument> for GraphApplyRequest {
    fn from(document: mixrelay_audio_graph::GraphDocument) -> Self {
        Self {
            document,
            processor_parameter_changes: Vec::new(),
            expected_graph_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCatalogEntry {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCatalogResponse {
    pub protocol_version: u16,
    pub catalog_revision: u64,
    pub active_id: String,
    pub graphs: Vec<GraphCatalogEntry>,
    pub document: mixrelay_audio_graph::GraphDocument,
    pub layout_revision: u64,
    pub layout: GraphLayout,
    /// Audio publication succeeded but disk replacement failed; never report a rollback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_error: Option<String>,
}

/// Presentation coordinates use UI identities, not DSP node identities. A duplex endpoint
/// may have two independently positioned UI cards backed by one runtime graph node.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphLayout {
    /// Display-only names for graph UI instances. Never used as application/device identities.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub node_labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub positions: std::collections::BTreeMap<String, GraphNodePosition>,
    /// User-resized cards keyed by UI identity; absent entries use their natural size.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub sizes: std::collections::BTreeMap<String, GraphNodeSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<GraphViewport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphNodePosition {
    pub x: f64,
    pub y: f64,
}

/// Explicit UI card dimensions in graph coordinates, independent of the audio document.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphNodeSize {
    /// Card width in graph pixels.
    pub width: f64,
    /// Card height in graph pixels.
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphViewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphLayoutRequest {
    pub expected_layout_revision: u64,
    pub layout: GraphLayout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphLayoutResponse {
    pub graph_id: String,
    pub layout_revision: u64,
    pub layout: GraphLayout,
}

/// Catalog edits fenced by both the execution and catalog revisions in the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum GraphCatalogAction {
    Create {
        name: String,
    },
    Select {
        id: String,
    },
    Rename {
        id: String,
        name: String,
    },
    /// Removes one authored graph and its execution owners. The final removal creates
    /// a new empty graph with a fresh identity, never reusing the removed target.
    Delete {
        id: String,
    },
    /// Restore the latest deleted graph with its original node and graph identities.
    UndoDelete,
    /// Delete the graph restored by the latest undo.
    RedoDelete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCatalogRequest {
    pub expected_revision: u64,
    pub expected_catalog_revision: u64,
    #[serde(flatten)]
    pub action: GraphCatalogAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverState {
    NotInstalled,
    Installing,
    RestartRequired,
    InstalledNotLoaded,
    Blocked,
    /// Installed transport cannot currently transfer audio; distinct from OS policy blocking.
    Unavailable,
    InvalidPermissions,
    VersionMismatch,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// Execution switch state; absent when the backend cannot control execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_power: Option<EnginePowerStatus>,
    /// Fresh per-processor transport samples; identities remain graph-local.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processor_timings: Vec<ProcessorTiming>,
    /// Prepared transport delay and cached native latency, independent of wall-clock timing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processor_latencies: Vec<ProcessorLatency>,
    /// Failed hosted processors that still belong to the active execution snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_processors: Vec<FailedProcessor>,
    /// Finalization error from a removed recorder, retained independently of its node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder_error: Option<String>,
    pub protocol_version: u16,
    pub driver_state: DriverState,
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
    pub underruns: u64,
    pub overruns: u64,
    /// Lifetime non-finite PCM replacements by Core boundary; no audio or node identities.
    pub sample_faults: mixrelay_audio_io::process::SampleFaults,
    /// `None` when the audio backend reports no session. UI must not manufacture
    /// capabilities when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_session: Option<mixrelay_audio_io::session::AudioSessionStatus>,
}

/// Requested execution state and whether native owners are still transitioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnginePowerStatus {
    pub enabled: bool,
    pub transitioning: bool,
    /// The service could not yet save this state for its next start; it keeps retrying.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsaved: bool,
    /// Why the engine paused itself; absent for a pause the user chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<EnginePauseReason>,
}

/// A pause the engine chose rather than the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnginePauseReason {
    /// It ended unexpectedly several times in a row; it stays paused until Resume.
    RepeatedFailures,
}

/// Idempotent execution command; never changes authored graphs or service installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnginePowerRequest {
    pub enabled: bool,
    /// Optional compare-and-set revision supplied by native capture preflight.
    pub expected_revision: Option<u64>,
}

/// One sampled block's engine-to-worker round trip, not hardware or plug-in reported latency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorTiming {
    pub graph_id: String,
    pub node_id: String,
    pub round_trip_ns: u64,
}

/// A live prepared processor's latency snapshot in its processing-rate sample frames.
///
/// This is observation only: it does not imply that graph paths are delay-compensated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorLatency {
    /// Authored graph identity, independent of the graph currently viewed in the editor.
    pub graph_id: String,
    /// Authored node identity within `graph_id`.
    pub node_id: String,
    /// Fixed engine-to-worker audio delay; excludes scheduling time and native latency.
    pub transport_frames: u32,
    /// Cached native latency; `None` is unknown, while `Some(0)` is a reported zero.
    pub intrinsic_frames: Option<u32>,
}

/// A failed active provider that can be explicitly retried against this exact worker instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailedProcessor {
    pub graph_id: String,
    pub node_id: String,
    /// Decimal `u64` token serialized as text to preserve exact identity in JavaScript clients.
    pub instance_id: String,
}

/// Explicit retry of one failed active provider, guarded by graph, catalog and worker identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessorRetryRequest {
    pub graph_id: String,
    pub node_id: String,
    pub expected_revision: u64,
    pub expected_catalog_revision: u64,
    pub expected_instance_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphApplyState {
    Pending,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRuntimeStatus {
    pub protocol_version: u16,
    pub active_revision: u64,
    pub submitted_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reason: Option<GraphPendingReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphPendingReason {
    AwaitingMicrophonePermission,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeMeterSnapshot {
    pub node_id: String,
    pub port_id: String,
    pub peaks: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamics: Option<DynamicsMeterSnapshot>,
}

/// Block peak input and actual maximum detector attenuation since the previous meter poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicsMeterSnapshot {
    pub input_peaks: Vec<f32>,
    pub gain_reduction_db: f32,
    pub threshold_db: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterSnapshot {
    pub protocol_version: u16,
    pub revision: u64,
    pub nodes: Vec<NodeMeterSnapshot>,
}

/// One pushed meter window. The engine incarnation identifies the engine process that
/// measured it: windows from a replaced engine never mix with its successor's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterTelemetry {
    pub engine_incarnation: String,
    /// Increases with every window this subscription delivers; gaps are allowed.
    pub sequence: u64,
    pub meters: MeterSnapshot,
}

#[cfg(test)]
mod dynamics_meter_contract_tests;
#[cfg(test)]
mod meter_telemetry_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalProcessorFormat {
    Au,
    Auv3,
    Clap,
    Lv2,
    Vst3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorParameterDescriptor {
    pub id: u32,
    pub name: String,
    pub default_value: f64,
    pub step_count: i32,
    pub can_automate: bool,
    pub is_read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalProcessorDescriptor {
    pub type_id: String,
    pub display_name: String,
    pub vendor: String,
    pub version: String,
    pub format: ExternalProcessorFormat,
    /// Exact native bus configurations validated by the disposable probe.
    pub layouts: Vec<mixrelay_audio_graph::ProviderPorts>,
    /// Preferred member of `layouts` for insertion without a manual selection.
    pub default_layout_id: String,
    pub has_editor: bool,
    pub parameters: Vec<ProcessorParameterDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalProcessorCatalog {
    pub protocol_version: u16,
    pub processors: Vec<ExternalProcessorDescriptor>,
    /// A partial scan retains previously usable entries and reports which discovery stages failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ExternalProcessorDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalProcessorDiagnostic {
    pub format: ExternalProcessorFormat,
    pub stage: ProcessorDiscoveryStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessorDiscoveryStage {
    Scan,
    Probe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorScanRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessorParameterUpdate {
    /// Revision observed when the caller chose this target.
    pub expected_revision: u64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorParameterUpdateResponse {
    pub protocol_version: u16,
    pub node_id: String,
    pub parameter_id: u32,
    pub value: f64,
    /// The native parameter was accepted even when durable graph storage failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessorEditorState {
    Unavailable,
    Closed,
    Opening,
    Open,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessorEditorFailure {
    NoEditor,
    DisplayUnavailable,
    HostWindowFailed,
    PluginRejected,
    WorkerUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessorEditorRequest {
    /// Revision observed when the caller chose this target.
    pub expected_revision: u64,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessorEditorStatus {
    pub protocol_version: u16,
    pub node_id: String,
    pub state: ProcessorEditorState,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub resizable: bool,
    pub failure: Option<ProcessorEditorFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphApplyResponse {
    pub protocol_version: u16,
    pub revision: u64,
    pub state: GraphApplyState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_error: Option<String>,
}

impl Default for AppStatus {
    fn default() -> Self {
        Self {
            engine_power: None,
            processor_timings: Vec::new(),
            processor_latencies: Vec::new(),
            failed_processors: Vec::new(),
            recorder_error: None,
            protocol_version: PROTOCOL_VERSION,
            driver_state: DriverState::NotInstalled,
            sample_rate: None,
            buffer_frames: None,
            underruns: 0,
            overruns: 0,
            sample_faults: mixrelay_audio_io::process::SampleFaults::default(),
            audio_session: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    DriverNotInstalled,
    DriverNotLoaded,
    DriverPermissionInvalid,
    DriverVersionMismatch,
    AudioDeviceDisconnected,
    /// Native input consent is outstanding; the proposed graph was not applied.
    AudioPermissionPending,
    /// Native input access was denied or restricted by the operating system.
    AudioPermissionDenied,
    BufferUnderrun,
    BufferOverrun,
    AudioSourceUnavailable,
    PairingRequired,
    Unauthorized,
    ControlUnavailable,
    /// The expected engine process has been replaced; the operation was not admitted.
    EngineReplaced,
    /// The client operation request header is malformed.
    InvalidOperationId,
    GraphInvalid,
    GraphRevisionStale,
    GraphExchangeBusy,
    GraphBindingUnsupported,
    /// The addressed authored node no longer exists.
    NodeNotFound,
    /// Processing is disabled; the command cannot act on runtime resources.
    NodeDisabled,
    ProcessorUnavailable,
    RecorderFailed,
    AudioSessionUnsupported,
    AudioSessionProcessorStateUnavailable,
    AudioSessionReconfigureFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub parameters: serde_json::Value,
}

#[cfg(test)]
mod engine_fence_tests;
#[cfg(test)]
mod tests;
