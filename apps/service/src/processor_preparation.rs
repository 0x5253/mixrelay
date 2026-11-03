//! Compiles portable plans and prepares DSP, hosted processors and recorder taps.
use mixrelay_audio_engine::RenderPlanError;

#[cfg(test)]
use crate::default_peak_meter_bank;
use crate::{
    Arc, BlockProcessor, CatalogNodeResolver, ExecutionPlan, ExternalProcessorFactory,
    GraphCompiler, NodeConfig, PeakMeterBank, PrepareProcessorError, ProcessorCatalog,
    ProcessorFactory, RenderPlan, recording_sinks::FileRecordingSinks,
};

pub(super) struct BuiltInProcessorFactory {
    pub(super) recorders: mixrelay_audio_recorder::RecorderRegistry,
    pub(super) catalog: ProcessorCatalog,
    pub(super) hosted: ExternalProcessorFactory,
    pub(super) meters: Arc<PeakMeterBank>,
    pub(super) render_prepare: fn(&ExecutionPlan) -> Result<RenderPlan, RenderPlanError>,
}

#[cfg(test)]
impl Default for BuiltInProcessorFactory {
    fn default() -> Self {
        Self {
            recorders: mixrelay_audio_recorder::RecorderRegistry::threaded_files(),
            catalog: ProcessorCatalog::default(),
            hosted: ExternalProcessorFactory::new(
                ProcessorCatalog::default(),
                std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            ),
            meters: default_peak_meter_bank(),
            render_prepare: RenderPlan::prepare,
        }
    }
}

impl ProcessorFactory for BuiltInProcessorFactory {
    fn compiler(&self) -> GraphCompiler {
        GraphCompiler::new(CatalogNodeResolver::new(self.catalog.clone()))
    }

    fn prepare(
        &self,
        execution: &ExecutionPlan,
    ) -> Result<Box<BlockProcessor>, PrepareProcessorError> {
        let mut processor = if execution
            .nodes()
            .iter()
            .any(|node| matches!(node.config(), NodeConfig::Provider(_)))
        {
            BlockProcessor::prepare_with_hosted_processors(
                execution,
                Arc::clone(&self.meters),
                &self.hosted,
            )
            .map_err(PrepareProcessorError::Render)?
        } else {
            let render = (self.render_prepare)(execution).map_err(PrepareProcessorError::Render)?;
            BlockProcessor::with_peak_meters(Arc::new(render), Arc::clone(&self.meters))
                .map_err(PrepareProcessorError::Render)?
        };
        processor
            .attach_recorders(&FileRecordingSinks(&self.recorders))
            .map_err(PrepareProcessorError::Recorder)?;
        Ok(Box::new(processor))
    }
}
