//! Compiles portable plans and prepares DSP, hosted processors and recorder taps.
use super::recording_sinks::FileRecordingSinks;
use mixrelay_audio_graph::{ExecutionPlan, GraphCompiler, NodeConfig};
use mixrelay_engine_core::{PrepareProcessorError, ProcessorFactory};
use mixrelay_processor_host::{CatalogNodeResolver, ExternalProcessorFactory, ProcessorCatalog};
use mixrelay_render::{BlockProcessor, PeakMeterBank, RenderPlan, RenderPlanError};
use std::sync::Arc;

pub(crate) struct BuiltInProcessorFactory {
    pub(crate) recorders: mixrelay_audio_recorder::RecorderRegistry,
    pub(crate) catalog: ProcessorCatalog,
    pub(crate) hosted: ExternalProcessorFactory,
    pub(crate) meters: Arc<PeakMeterBank>,
    pub(crate) render_prepare: fn(&ExecutionPlan) -> Result<RenderPlan, RenderPlanError>,
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
