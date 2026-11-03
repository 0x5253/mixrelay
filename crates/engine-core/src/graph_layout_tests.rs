use super::{Fixture, controller, request};
use crate::{GraphDocument, GraphResourceBackend, HostAudioBackendError, default_duplex_document};
use mixrelay_control_protocol::{
    ErrorCode, GraphCatalogAction, GraphLayout, GraphLayoutRequest, GraphNodePosition,
    GraphNodeSize, GraphViewport,
};
use std::sync::Arc;

fn layout(x: f64) -> GraphLayout {
    GraphLayout {
        node_labels: [
            ("default-virtual-io:input".into(), "영상용".into()),
            ("default-virtual-io:output".into(), "회의용".into()),
        ]
        .into(),
        positions: [
            (
                "default-virtual-io:input".into(),
                GraphNodePosition { x, y: 100.0 },
            ),
            (
                "default-virtual-io:output".into(),
                GraphNodePosition {
                    x: x + 400.0,
                    y: 300.0,
                },
            ),
        ]
        .into(),
        sizes: [(
            "default-virtual-io:input".into(),
            GraphNodeSize {
                width: 320.0,
                height: 220.0,
            },
        )]
        .into(),
        viewport: Some(GraphViewport {
            x: -80.0,
            y: 55.0,
            zoom: 0.75,
        }),
    }
}

#[test]
fn layout_sizes_reject_nonfinite_unbounded_or_invalid_identities() {
    for (width, height) in [
        (179.0, 26.0),
        (180.0, 25.0),
        (4097.0, 26.0),
        (180.0, 4097.0),
        (f64::NAN, 26.0),
        (180.0, f64::NAN),
        (f64::INFINITY, 26.0),
        (180.0, f64::NEG_INFINITY),
    ] {
        let mut value = GraphLayout::default();
        value
            .sizes
            .insert("node".into(), GraphNodeSize { width, height });
        assert_layout_validity(value, false);
    }
    for id in [String::new(), " ".into(), "x\n".into(), "x".repeat(257)] {
        let mut value = GraphLayout::default();
        value.sizes.insert(
            id,
            GraphNodeSize {
                width: 180.0,
                height: 26.0,
            },
        );
        assert_layout_validity(value, false);
    }
    let mut value = GraphLayout::default();
    for index in 0..4097 {
        value.sizes.insert(
            format!("node-{index}"),
            GraphNodeSize {
                width: 180.0,
                height: 26.0,
            },
        );
    }
    assert_layout_validity(value, false);
}

#[test]
fn layout_size_bounds_are_inclusive() {
    for (width, height) in [(180.0, 26.0), (4096.0, 4096.0)] {
        let mut value = GraphLayout::default();
        value
            .sizes
            .insert("node".into(), GraphNodeSize { width, height });
        assert_layout_validity(value, true);
    }
}

fn assert_layout_validity(layout: GraphLayout, valid: bool) {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let result = engine.update_graph_layout(
        "graph-1",
        GraphLayoutRequest {
            expected_layout_revision: 0,
            layout,
        },
    );
    if valid {
        assert!(result.is_ok());
    } else {
        assert_eq!(result.unwrap_err().code, ErrorCode::GraphInvalid);
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            before
        );
    }
}

#[test]
fn inactive_graph_layout_survives_selection_rename_and_restart() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let created = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Create {
                name: "SPAT".into(),
            },
        ))
        .unwrap();
    assert_eq!(created.layout, GraphLayout::default());
    assert_eq!(created.layout_revision, 0);
    // Viewing another graph has no DSP publication; layout remains presentation-only.
    let saved = engine
        .update_graph_layout(
            "graph-1",
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout(120.0),
            },
        )
        .unwrap();
    assert_eq!(saved.layout_revision, 1);
    assert_eq!(engine.graph_catalog().unwrap().active_id, created.active_id);
    assert_eq!(
        engine.graph_layout(&created.active_id).unwrap().layout,
        GraphLayout::default()
    );
    let selected = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Select {
                id: "graph-1".into(),
            },
        ))
        .unwrap();
    assert_eq!(selected.layout, saved.layout);
    assert_eq!(selected.layout_revision, 1);
    let renamed = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Rename {
                id: "graph-1".into(),
                name: "Ableton".into(),
            },
        ))
        .unwrap();
    assert_eq!(renamed.layout, saved.layout);
    drop(engine);
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(engine.graph_catalog().unwrap().layout, saved.layout);
    assert_eq!(engine.graph_catalog().unwrap().layout_revision, 1);
}

#[test]
fn layout_edit_never_prepares_native_resources_or_advances_audio_or_catalog_revision() {
    struct MustNotPrepare;
    impl GraphResourceBackend for MustNotPrepare {
        fn reconcile(
            &self,
            _: &GraphDocument,
            _: crate::PlaybackBindings<'_>,
        ) -> Result<(), HostAudioBackendError> {
            panic!("presentation changes must not prepare native resources");
        }
    }
    let fixture = Fixture::new();
    let mut engine = controller(default_duplex_document(1, Some(-12.0)))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    engine.graph_resources = Arc::new(MustNotPrepare);
    let document = engine.graph();
    let status = engine.graph_status();
    let catalog_revision = engine.graph_catalog().unwrap().catalog_revision;
    engine
        .update_graph_layout(
            "graph-1",
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout(-300.0),
            },
        )
        .unwrap();
    assert_eq!(engine.graph(), document);
    assert_eq!(engine.graph_status(), status);
    assert_eq!(
        engine.graph_catalog().unwrap().catalog_revision,
        catalog_revision
    );
    assert_eq!(engine.graph_catalog_document("graph-1").unwrap(), document);
}

#[test]
fn layout_revision_conflicts_are_independent_from_graph_edits_and_names() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    engine
        .update_graph_layout(
            "graph-1",
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout(100.0),
            },
        )
        .unwrap();
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let stale = engine
        .update_graph_layout(
            "graph-1",
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout(200.0),
            },
        )
        .unwrap_err();
    assert_eq!(stale.code, ErrorCode::GraphRevisionStale);
    assert_eq!(stale.parameters["resource"], "graph_layout");
    assert_eq!(stale.parameters["layoutRevision"], 1);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        before
    );
    engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Rename {
                id: "graph-1".into(),
                name: "renamed".into(),
            },
        ))
        .unwrap();
    let mut document = engine.graph();
    document.revision += 1;
    engine.apply_graph_request(document.clone().into()).unwrap();
    engine
        .update_graph_layout(
            "graph-1",
            GraphLayoutRequest {
                expected_layout_revision: 1,
                layout: layout(300.0),
            },
        )
        .unwrap();
    assert_eq!(engine.graph(), document);
    assert_eq!(engine.graph_layout("graph-1").unwrap().layout_revision, 2);
}

#[test]
fn layout_validation_rejects_unknown_ids_nonfinite_and_unbounded_values() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let mut invalid = vec![
        layout(f64::NAN),
        layout(f64::INFINITY),
        layout(-10_000_001.0),
    ];
    for zoom in [0.0, 8.1, f64::NAN] {
        let mut value = layout(0.0);
        value.viewport.as_mut().unwrap().zoom = zoom;
        invalid.push(value);
    }
    for id in [String::new(), " ".into(), "x\n".into(), "x".repeat(257)] {
        let mut value = GraphLayout::default();
        value
            .positions
            .insert(id, GraphNodePosition { x: 0.0, y: 0.0 });
        invalid.push(value);
    }
    let mut many = GraphLayout::default();
    for index in 0..4097 {
        many.positions.insert(
            format!("node-{index}"),
            GraphNodePosition { x: 0.0, y: 0.0 },
        );
    }
    invalid.push(many);
    for value in invalid {
        let error = engine
            .update_graph_layout(
                "graph-1",
                GraphLayoutRequest {
                    expected_layout_revision: 0,
                    layout: value,
                },
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::GraphInvalid);
        assert_eq!(error.parameters["resource"], "graph_layout");
    }
    assert!(engine.graph_layout("missing").is_err());
    assert!(
        engine
            .update_graph_layout(
                "missing",
                GraphLayoutRequest {
                    expected_layout_revision: 0,
                    layout: GraphLayout::default()
                }
            )
            .is_err()
    );
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        before
    );
}

#[test]
fn missing_or_corrupt_layouts_are_rejected_without_rewriting_the_catalog() {
    let fixture = Fixture::new();
    drop(
        controller(default_duplex_document(1, None))
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path()),
    );
    let saved: serde_json::Value = serde_json::from_slice(
        &crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
    )
    .unwrap();
    let mut missing = saved.clone();
    missing["graphs"][0]
        .as_object_mut()
        .unwrap()
        .remove("layoutRevision");
    let mut corrupt = saved;
    corrupt["graphs"][0]["layout"]["viewport"] = serde_json::json!({"x":0,"y":0,"zoom":-1});
    for catalog in [missing, corrupt] {
        let bytes = serde_json::to_vec(&catalog).unwrap();
        fixture.0.files.insert(fixture.path(), bytes.clone());
        let rejected = controller(default_duplex_document(1, None))
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        assert!(rejected.graph_catalog().is_err());
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            bytes
        );
    }
}

#[test]
fn display_labels_validate_without_changing_audio_identity_or_revision() {
    for label in ["", "   ", "line\nbreak", "tab\tname"] {
        let mut value = GraphLayout::default();
        value.node_labels.insert("node".into(), label.into());
        assert_layout_validity(value, false);
    }
    let mut long = GraphLayout::default();
    long.node_labels.insert("node".into(), "a".repeat(129));
    assert_layout_validity(long, false);
    let mut invalid_id = GraphLayout::default();
    invalid_id.node_labels.insert(" ".into(), "Video".into());
    assert_layout_validity(invalid_id, false);
    let many = GraphLayout {
        node_labels: (0..4097)
            .map(|id| (format!("node-{id}"), "name".into()))
            .collect(),
        ..GraphLayout::default()
    };
    assert_layout_validity(many, false);
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let before = engine.graph_catalog().unwrap();
    let saved = engine
        .update_graph_layout(
            &before.active_id,
            GraphLayoutRequest {
                expected_layout_revision: before.layout_revision,
                layout: layout(120.0),
            },
        )
        .unwrap();
    assert_eq!(
        saved.layout.node_labels["default-virtual-io:input"],
        "영상용"
    );
    assert_eq!(
        saved.layout.node_labels["default-virtual-io:output"],
        "회의용"
    );
    let after = engine.graph_catalog().unwrap();
    assert_eq!(before.document, after.document);
    assert_eq!(before.catalog_revision, after.catalog_revision);
    let mut reset = saved.layout;
    reset.node_labels.remove("default-virtual-io:input");
    let saved = engine
        .update_graph_layout(
            &before.active_id,
            GraphLayoutRequest {
                expected_layout_revision: saved.layout_revision,
                layout: reset,
            },
        )
        .unwrap();
    assert!(
        !saved
            .layout
            .node_labels
            .contains_key("default-virtual-io:input")
    );
    assert_eq!(
        saved.layout.node_labels["default-virtual-io:output"],
        "회의용"
    );
}

#[test]
fn control_snapshots_follow_display_names_without_changing_the_authored_node_id() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(9, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let catalog = engine.read_catalog().unwrap();
    let node_id = catalog.document.nodes[0].id.as_str().to_owned();
    let request = GraphLayoutRequest {
        expected_layout_revision: 0,
        layout: GraphLayout {
            node_labels: [(node_id.clone(), "Microphone".into())].into(),
            ..GraphLayout::default()
        },
    };
    engine
        .write_graph_layout(&catalog.active_id, request)
        .unwrap();
    let controls = engine.read_node_controls(&catalog.active_id).unwrap();
    let renamed = controls
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .unwrap();
    assert_eq!(renamed.label.as_deref(), Some("Microphone"));
    assert_eq!(engine.graph().revision, catalog.document.revision);
}

#[test]
fn disk_failures_keep_previous_layout_and_allow_same_revision_retry() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    for at_commit in [false, true] {
        if at_commit {
            fixture
                .0
                .files
                .reject_commit(fixture.path(), std::io::ErrorKind::IsADirectory);
        } else {
            fixture
                .0
                .files
                .reject_stage(fixture.path(), std::io::ErrorKind::NotADirectory);
        }
        let rejected = engine
            .update_graph_layout(
                "graph-1",
                GraphLayoutRequest {
                    expected_layout_revision: 0,
                    layout: layout(200.0),
                },
            )
            .unwrap_err();
        assert_eq!(rejected.code, ErrorCode::ControlUnavailable);
        assert_eq!(rejected.parameters["resource"], "graph_layout");
        assert_eq!(engine.graph_layout("graph-1").unwrap().layout_revision, 0);
        assert_eq!(
            engine.graph_layout("graph-1").unwrap().layout,
            GraphLayout::default()
        );
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            before
        );
        fixture.0.files.allow_stage(&fixture.path());
        fixture.0.files.allow_commit(&fixture.path());
    }
    fixture.0.files.allow_stage(&fixture.path());
    fixture.0.files.allow_commit(&fixture.path());
    engine
        .update_graph_layout(
            "graph-1",
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout(200.0),
            },
        )
        .unwrap();
    assert_eq!(engine.graph_layout("graph-1").unwrap().layout_revision, 1);
}
