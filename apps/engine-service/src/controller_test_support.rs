use super::*;

/// Acts as the audio owner's block-boundary acknowledgement and returns the
/// submitted processor for PCM assertions. Never starts a second preparation
/// while the controller is awaiting acknowledgement of its pending publication.
pub(crate) fn settle(controller: &EngineGraphController) -> Box<BlockProcessor> {
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
    let mut processor = BuiltInProcessorFactory::default().prepare(&plan).unwrap();
    assert!(controller.exchange.activate_pending(&mut processor));
    assert_eq!(processor.plan().revision(), revisions.submitted);
    controller.exchange.reclaim_retired();
    processor
}
