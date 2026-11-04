use super::*;

/// A native controller together with the composition handles its tests observe: the
/// processor exchange the audio owner would drain, the recorder registry and processor
/// state files, and the incarnation the composition issued.
pub(crate) struct ControllerFixture {
    pub(crate) engine: EngineGraphController,
    pub(crate) exchange: Arc<ProcessorExchange>,
    pub(crate) recorders: mixrelay_audio_recorder::RecorderRegistry,
    pub(crate) processor_files: ProcessorFiles,
    pub(crate) incarnation: String,
}

impl ControllerFixture {
    pub(crate) fn new(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
        document: GraphDocument,
    ) -> Self {
        let recorders = mixrelay_audio_recorder::RecorderRegistry::threaded_files();
        let processor_files = ProcessorFiles::default();
        let config = compose(config, recorders.clone(), processor_files.clone());
        let incarnation = config.incarnation.clone();
        let engine = EngineGraphController {
            core: mixrelay_engine_core::EngineGraphController::new_with_initial_document(
                Arc::clone(&exchange),
                config,
                document,
            ),
        };
        Self {
            engine,
            exchange,
            recorders,
            processor_files,
            incarnation,
        }
    }

    #[must_use]
    pub(crate) fn after_repeated_failures(mut self, tripped: bool) -> Self {
        self.engine = self.engine.after_repeated_failures(tripped);
        self
    }

    #[must_use]
    pub(crate) fn with_graph_catalog_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.engine = self.engine.with_graph_catalog_path(path);
        self
    }
}

impl std::ops::Deref for ControllerFixture {
    type Target = EngineGraphController;

    fn deref(&self) -> &EngineGraphController {
        &self.engine
    }
}

impl std::ops::DerefMut for ControllerFixture {
    fn deref_mut(&mut self) -> &mut EngineGraphController {
        &mut self.engine
    }
}

/// Acts as the audio owner's block-boundary acknowledgement and returns the
/// submitted processor for PCM assertions. Never starts a second preparation
/// while the controller is awaiting acknowledgement of its pending publication.
pub(crate) fn settle(controller: &ControllerFixture) -> Box<BlockProcessor> {
    let revisions = controller.exchange.revisions();
    assert_ne!(
        revisions.active, revisions.submitted,
        "no pending publication"
    );
    let plan = GraphCompiler::default()
        .compile(
            &GraphDocument::new(revisions.active, vec![], vec![]),
            controller.core.test_context(),
        )
        .unwrap();
    let mut processor = unhosted_factory().prepare(&plan).unwrap();
    assert!(controller.exchange.activate_pending(&mut processor));
    assert_eq!(processor.plan().revision(), revisions.submitted);
    controller.exchange.reclaim_retired();
    processor
}

/// Prepares built-in DSP and recorder taps without hosted processors or shared meters.
fn unhosted_factory() -> BuiltInProcessorFactory {
    BuiltInProcessorFactory {
        recorders: mixrelay_audio_recorder::RecorderRegistry::threaded_files(),
        catalog: ProcessorCatalog::default(),
        hosted: ExternalProcessorFactory::new(
            ProcessorCatalog::default(),
            Arc::new(mixrelay_time::ManualClock::default()),
        ),
        meters: default_peak_meter_bank(),
        render_prepare: RenderPlan::prepare,
    }
}
