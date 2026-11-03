//! Worker preparation consumed by catalog selection and exact-instance reuse policy.

use std::sync::Arc;

use mixrelay_audio_graph::{ProviderNodeConfig, RenderFormat};
use mixrelay_render::HostedAudioProcessor;

use crate::{HostedProcessorControl, catalog::CatalogEntry, control::ProcessorControlRegistration};

/// Worker creation failures before publication to a render plan.
#[derive(Debug, thiserror::Error)]
pub(super) enum WorkerPreparationError {
    #[error("{diagnostic}")]
    Native { diagnostic: String },
    #[error(transparent)]
    Identity(#[from] crate::instance_id::InstancesExhausted),
    #[error("prepared processor instance {observed} does not match issued instance {expected}")]
    InstanceMismatch {
        expected: crate::instance_id::InstanceId,
        observed: u64,
    },
}

pub(super) trait WorkerPreparation: Send + Sync {
    fn prepare(
        &self,
        entry: &CatalogEntry,
        config: &ProviderNodeConfig,
        format: RenderFormat,
        instance_id: crate::instance_id::InstanceId,
    ) -> Result<PreparedWorker, WorkerPreparationError>;
}

/// A prepared owner accepts its registration before publication to a render plan.
/// Dropping an unfinished owner must release its worker and all prepared resources.
pub(super) trait PreparedProcessor: Send {
    fn register(
        self: Box<Self>,
        registration: ProcessorControlRegistration,
    ) -> Box<dyn HostedAudioProcessor>;
}

pub(super) struct PreparedWorker {
    pub(super) processor: Box<dyn PreparedProcessor>,
    pub(super) control: Arc<HostedProcessorControl>,
}
