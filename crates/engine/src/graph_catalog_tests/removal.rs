use super::*;

fn remove(
    engine: &EngineGraphController,
    id: &str,
) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
    engine.mutate_graph_catalog(request(
        engine,
        GraphCatalogAction::Delete { id: id.into() },
    ))
}

#[test]
fn deleting_background_graph_republishes_without_changing_selected_graph_or_its_connections() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(9, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let first = engine.read_catalog().unwrap().active_id;
    let created = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Create {
                name: "Keep".into(),
            },
        ))
        .unwrap();
    let document = engine.graph();
    let response = remove(&engine, &first).unwrap();
    assert_eq!(response.active_id, created.active_id);
    assert_eq!(response.graphs.len(), 1);
    assert_eq!(response.document.nodes, document.nodes);
    assert_eq!(response.document.edges, document.edges);
    assert!(response.document.revision > document.revision);
    assert_ne!(
        engine.exchange.revisions().active,
        engine.exchange.revisions().submitted
    );
    settle(&engine);
    assert!(engine.read_catalog_document(&first).is_err());
    let disk: serde_json::Value = serde_json::from_slice(
        &crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
    )
    .unwrap();
    assert_eq!(disk["graphs"].as_array().unwrap().len(), 1);
    assert_eq!(disk["activeId"], response.active_id);
}

#[test]
fn deleting_selected_then_final_graph_uses_fresh_identity_and_rejects_late_commands() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(9, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let first = engine.read_catalog().unwrap();
    let second = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Create {
                name: "Remove".into(),
            },
        ))
        .unwrap();
    let response = remove(&engine, &second.active_id).unwrap();
    assert_eq!(response.active_id, first.active_id);
    assert_eq!(response.document.nodes, first.document.nodes);
    assert_eq!(response.document.edges, first.document.edges);
    settle(&engine);
    let response = remove(&engine, &first.active_id).unwrap();
    settle(&engine);
    assert_eq!(response.graphs.len(), 1);
    assert_ne!(response.active_id, first.active_id);
    assert_ne!(response.active_id, second.active_id);
    assert!(response.document.nodes.is_empty());
    assert!(response.document.edges.is_empty());
    assert!(engine.read_node_controls(&first.active_id).is_err());
    assert!(remove(&engine, &first.active_id).is_err());
    assert_eq!(engine.read_catalog().unwrap(), response);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one deletion history preserves ordered graph, node, layout and storage assertions"
)]
fn undo_redo_graph_deletion_restores_original_graph_nodes_and_layout_and_persists_them() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(9, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let original = engine.read_catalog().unwrap();
    let id = original.active_id.clone();
    let authored = mixrelay_control_protocol::GraphLayout {
        positions: [
            (
                "default-virtual-io:input".into(),
                mixrelay_control_protocol::GraphNodePosition { x: 137.0, y: 83.0 },
            ),
            (
                "default-virtual-io:output".into(),
                mixrelay_control_protocol::GraphNodePosition { x: 593.0, y: 271.0 },
            ),
        ]
        .into(),
        sizes: [(
            "default-virtual-io:input".into(),
            mixrelay_control_protocol::GraphNodeSize {
                width: 327.0,
                height: 219.0,
            },
        )]
        .into(),
        node_labels: [
            ("default-virtual-io:input".into(), "Programme input".into()),
            ("default-virtual-io:output".into(), "Monitor output".into()),
        ]
        .into(),
        viewport: Some(mixrelay_control_protocol::GraphViewport {
            x: -143.0,
            y: 67.0,
            zoom: 0.73,
        }),
    };
    let layout = engine
        .update_graph_layout(
            &id,
            mixrelay_control_protocol::GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: authored.clone(),
            },
        )
        .unwrap();
    assert_eq!(layout.layout, authored);
    remove(&engine, &id).unwrap();
    settle(&engine);
    assert!(engine.read_node_controls(&id).is_err());
    for _ in 0..2 {
        let restored = engine
            .mutate_graph_catalog(request(&engine, GraphCatalogAction::UndoDelete))
            .unwrap();
        settle(&engine);
        assert_eq!(restored.active_id, id);
        assert_eq!(restored.graphs, original.graphs);
        assert_eq!(restored.document.nodes, original.document.nodes);
        assert_eq!(engine.read_graph_layout(&id).unwrap().layout, layout.layout);
        assert!(engine.read_node_controls(&id).is_ok());
        engine
            .mutate_graph_catalog(request(&engine, GraphCatalogAction::RedoDelete))
            .unwrap();
        settle(&engine);
        assert!(engine.read_node_controls(&id).is_err());
    }
    engine
        .mutate_graph_catalog(request(&engine, GraphCatalogAction::UndoDelete))
        .unwrap();
    settle(&engine);
    let restarted = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(restarted.read_catalog().unwrap().active_id, id);
    let persisted = restarted.read_graph_layout(&id).unwrap();
    assert_eq!(persisted.layout, authored);
    assert_eq!(persisted.layout_revision, layout.layout_revision);
    assert_eq!(
        restarted.read_catalog().unwrap().document.nodes,
        original.document.nodes
    );
}

#[test]
fn failed_removal_preserves_catalog_disk_and_execution() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(9, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let before = engine.read_catalog().unwrap();
    let bytes = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let revisions = engine.exchange.revisions();
    let mut stale = request(
        &engine,
        GraphCatalogAction::Delete {
            id: before.active_id.clone(),
        },
    );
    stale.expected_catalog_revision += 1;
    assert!(engine.mutate_graph_catalog(stale).is_err());
    assert!(remove(&engine, "absent").is_err());
    assert_eq!(engine.read_catalog().unwrap(), before);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        bytes
    );
    assert_eq!(engine.exchange.revisions(), revisions);
    fixture
        .0
        .files
        .reject_commit(fixture.path(), std::io::ErrorKind::IsADirectory);
    let response = remove(&engine, &before.active_id).unwrap();
    // A post-publication replacement error is an accepted edit with a persistence warning.
    assert!(response.persistence_error.is_some());
    settle(&engine);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        bytes
    );
}
