//! Native file/child composition for the processor preparation port.

use std::sync::Arc;

use mixrelay_audio_engine::HostedAudioProcessor;
use mixrelay_audio_graph::{ProviderNodeConfig, RenderFormat};

use crate::{
    ExternalProcessorFactory, ProcessorCatalog, ProcessorControlRegistry, ProcessorFiles,
    catalog::CatalogEntry,
    control::ProcessorControlRegistration,
    hosted::{HostedExternalProcessor, NativeInstance},
    preparation::{PreparedProcessor, PreparedWorker, WorkerPreparation, WorkerPreparationError},
};

impl ExternalProcessorFactory {
    /// Selects native worker preparation with no durable state namespace yet.
    /// The clock is sampled on requested audio blocks; `now` must not allocate or block.
    #[must_use]
    pub fn new(catalog: ProcessorCatalog, clock: Arc<dyn mixrelay_time::MonotonicClock>) -> Self {
        let files = ProcessorFiles::default();
        let controls = ProcessorControlRegistry::new(Arc::new(files.clone()));
        Self::with_controls(catalog, controls, files, clock)
    }

    /// Composes native worker preparation with the selected controls and file owner.
    /// The clock is sampled on requested audio blocks; `now` must not allocate or block.
    #[must_use]
    pub fn with_controls(
        catalog: ProcessorCatalog,
        controls: ProcessorControlRegistry,
        files: ProcessorFiles,
        clock: Arc<dyn mixrelay_time::MonotonicClock>,
    ) -> Self {
        Self::with_preparation(
            catalog,
            controls,
            Arc::new(NativePreparation { files, clock }),
        )
    }
}

struct NativePreparation {
    files: ProcessorFiles,
    clock: Arc<dyn mixrelay_time::MonotonicClock>,
}

impl WorkerPreparation for NativePreparation {
    fn prepare(
        &self,
        entry: &CatalogEntry,
        config: &ProviderNodeConfig,
        format: RenderFormat,
        instance_id: crate::instance_id::InstanceId,
    ) -> Result<PreparedWorker, WorkerPreparationError> {
        let (processor, control) = HostedExternalProcessor::new(
            entry,
            config,
            format,
            self.files.optional_snapshot().as_ref(),
            NativeInstance {
                id: instance_id,
                clock: Arc::clone(&self.clock),
            },
        )
        .map_err(|diagnostic| WorkerPreparationError::Native { diagnostic })?;
        Ok(PreparedWorker {
            processor: Box::new(processor),
            control,
        })
    }
}

impl PreparedProcessor for HostedExternalProcessor {
    fn register(
        mut self: Box<Self>,
        registration: ProcessorControlRegistration,
    ) -> Box<dyn HostedAudioProcessor> {
        self.registration = Some(registration);
        self
    }
}
