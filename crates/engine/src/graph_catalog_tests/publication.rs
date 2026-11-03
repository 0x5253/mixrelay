use super::*;
use mixrelay_processor_state::{StateBlobSink, StateBlobSource};
use sha2::{Digest, Sha256};

#[test]
fn three_large_state_references_persist_and_reload_before_processor_prepare() {
    const STATE_BYTES: usize = 9 * 1024 * 1024;
    let fixture = Fixture::new();
    let (engine, prepares) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let revisions = engine.exchange.revisions();
    let mut document = parameter_document();
    document.revision = revisions.submitted + 1;
    let template = document.nodes[0].clone();
    let store = engine.processor_files.snapshot().unwrap();
    let references = [0x35_u8, 0x76, 0xa9].map(|value| {
        let bytes = vec![value; STATE_BYTES];
        let reference = mixrelay_processor_state::StateRef::new(
            format!("{:x}", Sha256::digest(&bytes)),
            bytes.len() as u64,
        )
        .unwrap();
        store
            .store_verified(&mut std::io::Cursor::new(bytes), &reference)
            .unwrap();
        reference
    });
    document.nodes = references
        .iter()
        .enumerate()
        .map(|(index, reference)| {
            let mut node = template.clone();
            node.id = NodeId::new(format!("effect-{index}"));
            let NodeConfig::Provider(provider) = &mut node.config else {
                panic!("provider fixture")
            };
            provider.parameters = serde_json::json!({"stateRef": reference});
            node
        })
        .collect();
    engine
        .apply_graph_locked(document.clone(), engine.catalog_candidate(&document))
        .unwrap();
    assert_eq!(prepares.load(Ordering::Relaxed), 1);
    assert_eq!(engine.exchange.revisions().submitted, document.revision);
    assert_eq!(engine.graph(), document);
    for reference in &references {
        store.verify(reference).unwrap();
    }
    assert!(fixture.0.files.bytes(&fixture.path()).unwrap().len() < 16 * 1024);

    let states = engine.processor_files.clone();
    drop(engine);
    let (mut restarted, restarted_prepares) =
        parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    restarted.processor_files = states;
    let restarted = restarted
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(restarted_prepares.load(Ordering::Relaxed), 1);
    assert_eq!(restarted.graph().nodes.len(), 3);
    let restarted_store = restarted.processor_files.snapshot().unwrap();
    for reference in &references {
        restarted_store.verify(reference).unwrap();
    }
    for (node, reference) in restarted.graph().nodes.iter().zip(references) {
        let NodeConfig::Provider(provider) = &node.config else {
            panic!("provider fixture")
        };
        assert_eq!(
            provider.parameters["stateRef"],
            serde_json::to_value(reference).unwrap()
        );
    }
}

#[test]
fn unavailable_catalog_storage_rejects_before_prepare_and_same_revision_can_retry() {
    let fixture = Fixture::new();
    let (engine, prepares) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let disk = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let revisions = engine.exchange.revisions();
    fixture
        .0
        .files
        .reject_stage(fixture.path(), std::io::ErrorKind::NotADirectory);
    let mut document = parameter_document();
    document.revision = revisions.submitted + 1;
    assert!(
        engine
            .apply_graph_locked(document.clone(), engine.catalog_candidate(&document))
            .is_err()
    );
    assert_eq!(engine.exchange.revisions(), revisions);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        disk
    );
    assert_eq!(prepares.load(Ordering::Relaxed), 0);

    fixture.0.files.allow_stage(&fixture.path());
    engine
        .apply_graph_locked(document.clone(), engine.catalog_candidate(&document))
        .unwrap();
    assert_eq!(prepares.load(Ordering::Relaxed), 1);
    assert_eq!(engine.graph(), document);
}
