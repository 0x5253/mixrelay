//! Real staging cleanup and joined maintenance ownership at the application boundary.
use crate::*;
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

struct CountingFactory {
    base: Arc<dyn ProcessorFactory>,
    prepares: Arc<AtomicUsize>,
}
impl ProcessorFactory for CountingFactory {
    fn compiler(&self) -> GraphCompiler {
        self.base.compiler()
    }
    fn prepare(&self, plan: &ExecutionPlan) -> Result<Box<BlockProcessor>, PrepareProcessorError> {
        self.prepares.fetch_add(1, Ordering::Relaxed);
        self.base.prepare(plan)
    }
}

#[test]
fn invalid_graph_after_successful_staging_removes_only_its_temporary_document() {
    let fixture = mixrelay_test_support::FixtureDir::new("catalog-cleanup").unwrap();
    let path = fixture.join("graphs.json");
    let mut engine = endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]))
        .with_graph_catalog_path(path.clone());
    let prepares = Arc::new(AtomicUsize::new(0));
    let base = engine.core.test_factory();
    engine.core.test_set_factory(Arc::new(CountingFactory {
        base,
        prepares: prepares.clone(),
    }));
    let disk = fs::read(&path).unwrap();
    let revisions = engine.exchange.revisions();
    let document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2, "revision":revisions.submitted + 1,
        "nodes":[
            {"id":"duplicate","type":"gain","config":{"channels":2,"gainDb":0.0}},
            {"id":"duplicate","type":"gain","config":{"channels":2,"gainDb":0.0}}
        ], "edges":[]
    }))
    .unwrap();
    assert!(engine.core.test_publish_staged_graph(document).is_err());
    assert_eq!(engine.exchange.revisions(), revisions);
    assert_eq!(fs::read(&path).unwrap(), disk);
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
    assert_eq!(prepares.load(Ordering::Relaxed), 0);
}

#[test]
fn providerless_pending_graph_needs_no_snapshot_or_shutdown_checkpoint() {
    let fixture = mixrelay_test_support::FixtureDir::new("catalog-maintenance").unwrap();
    let path = fixture.join("graphs.json");
    let engine = endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]))
        .with_graph_catalog_path(path.clone());
    let mut document = engine.graph();
    document.revision += 1;
    engine.apply_graph_request(document.clone().into()).unwrap();
    let revisions = engine.exchange.revisions();
    assert_ne!(revisions.active, revisions.submitted);
    let disk = fs::read(&path).unwrap();
    engine.core.test_checkpoint_without_providers().unwrap();
    let exchange = Arc::clone(&engine.exchange);
    let engine = Arc::new(engine.engine);
    engine
        .start_control_maintenance()
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(exchange.revisions(), revisions);
    assert_eq!(engine.graph(), document);
    assert_eq!(fs::read(&path).unwrap(), disk);
    assert!(engine.graph_catalog().unwrap().persistence_error.is_none());
}
