//! Prepared DSP plans consumed by engine publication transactions.
use mixrelay_audio_graph::{ExecutionPlan, GraphCompiler};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};
use mixrelay_render::{BlockProcessor, RecordingSinkError, RenderPlanError};

pub trait ProcessorFactory: Send + Sync {
    fn compiler(&self) -> GraphCompiler;
    /// # Errors
    /// Reports DSP preparation or recording sink admission failure.
    fn prepare(
        &self,
        execution: &ExecutionPlan,
    ) -> Result<Box<BlockProcessor>, PrepareProcessorError>;
}
#[derive(Debug, thiserror::Error)]
pub enum PrepareProcessorError {
    #[error(transparent)]
    Render(#[from] RenderPlanError),
    #[error("{0}")]
    Recorder(#[source] RecordingSinkError),
}
pub(crate) fn prepare_error(error: PrepareProcessorError) -> ProtocolError {
    match error {
        PrepareProcessorError::Recorder(error) => crate::recorder_sessions::recorder_error(error.0),
        PrepareProcessorError::Render(error) => ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({"renderError":error.to_string()}),
        },
    }
}
