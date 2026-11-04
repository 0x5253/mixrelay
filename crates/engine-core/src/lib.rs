//! Engine domain transactions and ports consumed by native runtime owners.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioEndpoint, ChannelLayout, ChannelMapping, CompileContext, CompileLimits,
    EdgeId, EdgeSpec, FROM_OS_PORT_ID, GainConfig, GraphDocument, NodeConfig, NodeId, NodeSpec,
    PortId, RenderFormat, StreamSpec, TO_OS_PORT_ID, VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_audio_io::{
    HostAudioBackendError, VirtualEndpointBackend, VirtualEndpointCapabilities,
};
use mixrelay_control_protocol::{
    AppStatus, DriverState, ErrorCode, ExternalProcessorCatalog, GraphApplyRequest,
    GraphApplyResponse, GraphApplyState, GraphRuntimeStatus, MeterSnapshot, NodeMeterSnapshot,
    PROTOCOL_VERSION, ProcessorEditorStatus, ProcessorParameterUpdateResponse,
    ProcessorRetryRequest, ProcessorScanRequest, ProtocolError, SessionReconfigureRequest,
};
use mixrelay_render::{
    ExternalBinding, MeterBinding, PeakMeterBank, ProcessorConfigurationError, ProcessorExchange,
    ProcessorSubmitError, RenderPlan,
};
use parking_lot::{Mutex, RwLock};

use endpoint_bindings::{validate_endpoint_bindings, virtual_endpoint_specs};
use mixrelay_audio_graph::composition as graph_composition;
use processor_control::{
    persist_processor_parameter, processor_editor_error, processor_parameter_error,
};
use processor_ports::{ProcessorCatalog, ProcessorControls, ProcessorStates};
use processor_preparation::prepare_error;
use session_change::{SessionChangeOrigin, SessionPauseOwner, session_error};
include!("controller.rs");
mod processor_preparation;
pub use processor_preparation::PrepareProcessorError;
pub use processor_preparation::ProcessorFactory;
mod default_graph;
pub use default_graph::{default_duplex_document, default_session_document};
mod runtime_backend;
pub use runtime_backend::{
    PreparedSessionFormat, ProcessingControlBackend, RuntimeBackend, RuntimeStatusBackend,
    SessionFormatBackend,
};
pub mod catalog_execution;
mod control_api;
mod control_telemetry;
pub mod document_files;
mod endpoint_bindings;
mod endpoint_format;
mod engine_power;
mod execution_choice;
mod execution_control;
mod graph_publication;
mod meter_readback;
mod meter_snapshot;
mod native_resources;
mod node_control;
mod node_control_snapshot;
mod processor_control;
mod processor_latency;
pub mod processor_ports;
mod processor_recovery;
mod processor_timing;
mod recorder_control;
mod recorder_sessions;
pub mod recording_ports;
pub mod render_demand;
pub mod resource_requirements;
mod runtime_node_control;
pub mod session;
mod session_change;
pub mod session_settings;
pub mod worker_completion;
pub use meter_readback::default_peak_meter_bank;
mod graph_catalog;
pub use native_resources::{
    ContractOnlyVirtualEndpointBackend, FixedRuntimeStatus, GraphResourceBackend,
    NoopGraphResourceBackend, PlaybackBindings, PreparedGraphFormat,
};
pub const DEFAULT_ENDPOINT_NODE_ID: &str = "default-virtual-io";

pub mod maintenance;

pub mod crash_policy;

#[cfg(test)]
mod controller_test_support;
#[cfg(test)]
mod endpoint_settings_tests;
#[cfg(any(test, feature = "test-util"))]
mod test_support;
#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;

#[cfg(feature = "test-util")]
pub mod test_util;

#[cfg(test)]
mod controller_startup_tests;
#[cfg(test)]
mod endpoint_format_tests;
#[cfg(test)]
mod session_change_tests;

#[cfg(test)]
mod graph_catalog_tests;

#[cfg(test)]
mod endpoint_persistence_tests;
#[cfg(test)]
mod virtual_source_gain_tests;
