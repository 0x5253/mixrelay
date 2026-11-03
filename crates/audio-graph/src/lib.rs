mod builtin;
mod compiler;
pub mod composition;
mod effects;
mod issues;
mod model;
mod provider_layout;
mod resolution;

#[cfg(test)]
mod tests;

pub use builtin::{BuiltInNodeResolver, validate_virtual_endpoint_gain};
pub use compiler::GraphCompiler;
pub use effects::{
    ChannelMode, ChannelParameters, ChorusParameters, DelayMode, DelayParameters,
    DuckingParameters, DynamicsParameters, Effect, EffectConfig, EffectModeError,
    EffectValidationError, EqualizerParameters, FilterParameters, PositionParameters,
    ReverbAlgorithm, ReverbParameters, SIDECHAIN_PORT_ID,
};
pub use issues::{CompileError, GraphIssue, GraphIssueCode, GraphResource};
pub use mixrelay_audio_io::{
    ApplicationCaptureRequest, ApplicationSelector, ChannelLayout, SourcePlaybackPolicy,
    StreamSpec, VirtualEndpointId, VirtualEndpointSpec,
};
pub use model::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint,
    CURRENT_GRAPH_SCHEMA_VERSION, ChannelMapping, ChannelRoute, CompiledEdge, CompiledNode,
    CompiledPort, CompressorConfig, DeviceSelector, EdgeId, EdgeSpec, ExecutionPlan,
    FROM_OS_PORT_ID, GainConfig, GraphDocument, LimiterConfig, MixerConfig, NodeConfig,
    NodeControl, NodeId, NodeKind, NodeSpec, NodeTypeId, PortDirection, PortId, ProviderNodeConfig,
    RenderFormat, TO_OS_PORT_ID, VirtualEndpointConfig, VirtualEndpointReference,
};
pub use provider_layout::{
    MAX_PROVIDER_BUS_ID_BYTES, MAX_PROVIDER_BUS_NAME_BYTES, MAX_PROVIDER_BUSES_PER_DIRECTION,
    MAX_PROVIDER_CHANNELS_PER_DIRECTION, MAX_PROVIDER_LAYOUT_ID_BYTES, ProviderBus,
    ProviderBusKind, ProviderChannelFormat, ProviderGeometryError, ProviderPorts,
    ProviderPortsError, validate_provider_geometry,
};
pub use resolution::{
    CompileContext, CompileLimits, NodeResolutionError, NodeResolver, ResolvedNode,
};
