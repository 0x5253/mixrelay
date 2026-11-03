//! Prepared audio rendering, block-boundary publication and metering contracts.
pub mod content_latency;
mod dsp;
mod latency_exchange;
mod latency_plan;
mod measured_arrivals;
mod meters;
mod processor_exchange;
mod render;
mod runtime_control;

pub(crate) use meters::DynamicsMeterBlock;
pub use meters::{DynamicsMeterReading, MeterPublication, PeakMeterBank};
pub use processor_exchange::{
    ProcessorConfigurationError, ProcessorExchange, ProcessorRevisions, ProcessorSubmitError,
};
pub use render::{
    ArrivalError, BlockProcessor, CompensationState, CycleAdmission, ExternalBinding,
    ExternalControlState, ExternalIo, HostedAudioProcessor, HostedLatencySource,
    HostedProcessorFactory, HostedProcessorIdentity, HostedProcessorPrepareRequest, MeterBinding,
    ProcessBlockError, RecordingSink, RecordingSinkError, RecordingSinks, RenderPlan,
    RenderPlanError, recording_session_key,
};
pub use runtime_control::{RuntimeControlBank, RuntimeControlError, RuntimeControlUpdate};
