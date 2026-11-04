//! The control server reaches the native controller only through `ControlBackend`, so a
//! preview must arrive at a read and an update at the mutation it names.
use super::*;
use mixrelay_control_protocol::NodeControlRequest;

fn gain_graph() -> GraphDocument {
    serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,
        "nodes":[{"id":"gain","type":"gain","config":{"channels":2,"gainDb":-3.0}}],
        "edges":[]
    }))
    .unwrap()
}

fn gain_muted(backend: &dyn ControlBackend, graph_id: &str) -> bool {
    backend
        .node_controls(graph_id)
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.node_id == "gain")
        .unwrap()
        .control
        .muted
}

#[test]
fn node_control_preview_leaves_the_graph_and_update_applies_it() {
    let directory = mixrelay_test_support::FixtureDir::new("control-api-node").unwrap();
    let engine = controller(gain_graph()).with_graph_catalog_path(directory.join("graphs.json"));
    let backend: &dyn ControlBackend = &*engine;
    let graph_id = backend.graph_catalog().unwrap().active_id;
    let before = backend.graph();
    let mute = NodeControlRequest {
        expected_revision: engine.exchange.revisions().submitted,
        enabled: None,
        muted: Some(true),
        gain_db: None,
    };

    backend
        .preview_node_control(&graph_id, "gain", mute.clone())
        .unwrap();
    assert_eq!(backend.graph(), before);
    assert!(!gain_muted(backend, &graph_id));

    backend
        .update_node_control(&graph_id, "gain", mute)
        .unwrap();
    assert!(gain_muted(backend, &graph_id));
}

#[test]
fn catalog_reads_return_the_active_document_without_publishing() {
    let directory = mixrelay_test_support::FixtureDir::new("control-api-catalog").unwrap();
    let engine = controller(gain_graph()).with_graph_catalog_path(directory.join("graphs.json"));
    let backend: &dyn ControlBackend = &*engine;
    let revisions = engine.exchange.revisions();
    let catalog = backend.graph_catalog().unwrap();

    assert_eq!(
        backend.graph_catalog_document(&catalog.active_id).unwrap(),
        catalog.document
    );
    assert_eq!(
        backend.graph_layout(&catalog.active_id).unwrap().graph_id,
        catalog.active_id
    );
    assert_eq!(engine.exchange.revisions(), revisions);
}
