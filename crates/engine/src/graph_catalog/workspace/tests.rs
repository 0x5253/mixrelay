use std::path::Path;

use super::*;
use crate::{
    controller_test_support::settle,
    endpoint_settings_tests::{controller, controller_with_inventory},
};
use mixrelay_audio_graph::{NodeSpec, ProviderNodeConfig};
use mixrelay_processor_state::{StateBlobSource, StateRef, write_bundle};

fn request(engine: &EngineGraphController, operation: WorkspaceAction) -> WorkspaceRequest {
    let catalog = engine.read_catalog().unwrap();
    WorkspaceRequest {
        file_digest: None,
        expected_revision: catalog.document.revision,
        expected_catalog_revision: catalog.catalog_revision,
        operation,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the fixture independently specifies native destination, manifest version and bundle assets"
)]
fn write_bundle_fixture(
    files: &crate::document_files::memory::MemoryDocuments,
    path: &Path,
    catalog: Catalog,
    references: &[StateRef],
    store: &dyn StateBlobSource,
    version: u32,
) {
    let manifest = serde_json::to_vec(&WorkspaceFile {
        format: "MixRelay".into(),
        version,
        catalog,
    })
    .unwrap();
    let mut bytes = Vec::new();
    write_bundle(&mut bytes, &manifest, references, store).unwrap();
    files.insert(path, bytes);
}

fn add_unavailable_provider(document: &mut GraphDocument) {
    document.nodes.push(NodeSpec {
        id: "missing-plugin".into(),
        label: Some("Unavailable effect".into()),
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::Provider(ProviderNodeConfig {
            type_id: "vst3:not-installed".into(),
            parameters: serde_json::json!({"parameterValues":{"7":0.75}}),
            ports: None,
        }),
    });
}

#[test]
fn save_preserves_missing_driver_and_plugin_work_without_execution_admission() {
    let dir = crate::test_support::Documents::new("unavailable-export");
    let mut document = crate::default_duplex_document(1, None);
    add_unavailable_provider(&mut document);
    let empty = GraphDocument::new(1, vec![], vec![]);
    let engine = controller_with_inventory(document.clone(), &empty);

    let engine = engine
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(dir.join("graphs.json"));
    let before = engine.read_catalog().unwrap();
    let path = dir.join("preserved.mix");
    engine
        .workspace_operation(request(
            &engine,
            WorkspaceAction::Save {
                path: path.to_string_lossy().into_owned(),
            },
        ))
        .unwrap();
    assert_eq!(
        load(&dir.files, &path)
            .unwrap()
            .graphs
            .active()
            .document
            .nodes,
        document.nodes
    );
    assert_eq!(
        load(&dir.files, &path)
            .unwrap()
            .graphs
            .active()
            .document
            .edges,
        document.edges
    );
    assert_eq!(engine.read_catalog().unwrap(), before);
    assert_eq!(
        Catalog::load(&dir.files, &dir.join("graphs.json"))
            .unwrap()
            .unwrap()
            .graphs
            .active()
            .document
            .nodes,
        document.nodes
    );
    assert!(
        engine
            .preview_workspace(request(
                &engine,
                WorkspaceAction::Open {
                    path: path.to_string_lossy().into_owned(),
                }
            ))
            .is_err()
    );
    assert_eq!(engine.read_catalog().unwrap(), before);
}

#[test]
fn save_does_not_require_a_disconnected_hardware_output_to_be_available() {
    let dir = crate::test_support::Documents::new("hardware-export");
    let document = GraphDocument::new(
        1,
        vec![NodeSpec {
            id: "output".into(),
            label: Some("Disconnected interface".into()),
            control: mixrelay_audio_graph::NodeControl::default(),
            config: NodeConfig::AudioDeviceOutput(mixrelay_audio_graph::AudioDeviceOutputConfig {
                selector: mixrelay_audio_io::DeviceSelector::PersistentUid {
                    uid: "offline-interface".into(),
                },
                input_channels: 2,
            }),
        }],
        vec![],
    );
    let engine = controller(document.clone())
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(dir.join("graphs.json"));
    let path = dir.join("hardware.mix");
    engine
        .workspace_operation(request(
            &engine,
            WorkspaceAction::Save {
                path: path.to_string_lossy().into_owned(),
            },
        ))
        .unwrap();
    assert_eq!(
        load(&dir.files, &path).unwrap().graphs.active().document,
        document
    );
}

#[test]
fn single_graph_can_save_open_and_restore_its_routing_and_layout_after_restart() {
    let dir = crate::test_support::Documents::new("workspace-routing");
    let catalog_path = dir.join("graphs.json");
    let initial = crate::default_duplex_document(9, Some(-6.0));
    let engine = controller(initial.clone())
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(catalog_path.clone());
    let catalog = engine.read_catalog().unwrap();
    let mut layout = mixrelay_control_protocol::GraphLayout::default();
    layout.positions.insert(
        initial.nodes[0].id.as_str().into(),
        mixrelay_control_protocol::GraphNodePosition { x: 125.0, y: 250.0 },
    );
    layout.node_labels.insert(
        format!("{}:input", initial.nodes[0].id.as_str()),
        "회의용".into(),
    );
    engine
        .update_graph_layout(
            &catalog.active_id,
            mixrelay_control_protocol::GraphLayoutRequest {
                expected_layout_revision: catalog.layout_revision,
                layout: layout.clone(),
            },
        )
        .unwrap();
    let path = dir.join("routing.mix").to_string_lossy().into_owned();
    engine
        .workspace_operation(request(
            &engine,
            WorkspaceAction::Save { path: path.clone() },
        ))
        .unwrap();
    engine
        .workspace_operation(request(&engine, WorkspaceAction::New))
        .unwrap();
    let _processor = settle(&engine);
    assert!(engine.graph().nodes.is_empty());
    let open = request(&engine, WorkspaceAction::Open { path });
    let preview = engine.preview_workspace(open.clone()).unwrap();
    let mut open = open;
    open.file_digest = preview.file_digest;
    let loaded = engine.workspace_operation(open).unwrap();
    let _processor = settle(&engine);
    assert_eq!(loaded.document.nodes, initial.nodes);
    assert_eq!(loaded.document.edges, initial.edges);
    assert_eq!(loaded.layout, layout);
    drop(engine);
    let restarted = controller_with_inventory(GraphDocument::new(1, vec![], vec![]), &initial);

    let restarted = restarted
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(catalog_path);
    let _processor = settle(&restarted);
    let restored = restarted.read_catalog().unwrap();
    assert_eq!(restored.graphs.len(), 1);
    assert_eq!(restored.document.nodes, initial.nodes);
    assert_eq!(restored.document.edges, initial.edges);
    assert_eq!(restored.layout, layout);
}

#[test]
fn startup_restores_multiple_graphs_and_persists_layout_edits() {
    let dir = crate::test_support::Documents::new("multiple-graph-catalog");
    let path = dir.join("graphs.json");
    let mut catalog = Catalog::imported(GraphDocument::new(1, vec![], vec![]));
    let mut second = catalog.graphs.active().clone();
    second.id = "graph-2".into();
    catalog.graphs.push(second);
    let bytes = serde_json::to_vec(&catalog).unwrap();
    dir.files.insert(&path, bytes.clone());
    let engine = controller(GraphDocument::new(1, vec![], vec![]))
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(path.clone());
    let selected = engine.read_catalog().unwrap();
    assert_eq!(selected.graphs.len(), 2);
    engine
        .update_graph_layout(
            &selected.active_id,
            mixrelay_control_protocol::GraphLayoutRequest {
                expected_layout_revision: selected.layout_revision,
                layout: mixrelay_control_protocol::GraphLayout::default(),
            },
        )
        .unwrap();
    assert_ne!(dir.files.bytes(&path).unwrap(), bytes);
    assert_eq!(
        Catalog::load(&dir.files, &path)
            .unwrap()
            .unwrap()
            .graphs
            .len(),
        2
    );
}

#[test]
fn save_open_preserves_catalog_ids_and_layout_and_advances_revisions() {
    let dir = crate::test_support::Documents::new("workspace");
    let engine = controller(GraphDocument::new(9, vec![], vec![]))
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(dir.join("graphs.json"));
    let path = dir.join("session.mix").to_string_lossy().into_owned();
    let before = engine.read_catalog().unwrap();
    engine
        .workspace_operation(request(
            &engine,
            WorkspaceAction::Save { path: path.clone() },
        ))
        .unwrap();
    let file = load(&dir.files, Path::new(&path)).unwrap();
    assert_eq!(file.selected_id(), before.active_id);
    engine
        .workspace_operation(request(&engine, WorkspaceAction::New))
        .unwrap();
    let _processor = settle(&engine);
    let result = engine
        .workspace_operation(request(&engine, WorkspaceAction::Open { path }))
        .unwrap();
    assert_eq!(result.graphs, before.graphs);
    assert_eq!(result.layout, before.layout);
    assert!(result.document.revision > before.document.revision);
    assert!(result.catalog_revision > before.catalog_revision);
}

#[test]
fn invalid_binary_manifest_and_json_only_file_leave_workspace_untouched() {
    let dir = crate::test_support::Documents::new("workspace-invalid");
    let engine = controller(GraphDocument::new(9, vec![], vec![]))
        .with_test_documents(&dir.files)
        .with_graph_catalog_path(dir.join("graphs.json"));
    let before = engine.read_catalog().unwrap();
    let graph_bytes = dir.files.bytes(&dir.join("graphs.json")).unwrap();
    let catalog: serde_json::Value = serde_json::from_slice(&graph_bytes).unwrap();
    let empty_store = crate::test_support::MemoryAssets::default();
    let unknown_version = dir.join("unknown.mix");
    write_bundle_fixture(
        &dir.files,
        &unknown_version,
        serde_json::from_value(catalog.clone()).unwrap(),
        &[],
        &empty_store,
        99,
    );
    let json_only = dir.join("json-only.mix");
    dir.files.insert(
        &json_only,
        serde_json::json!({"format":"MixRelay","version":1,"catalog":catalog}).to_string(),
    );
    let truncated = dir.join("truncated.mix");
    dir.files.insert(&truncated, b"MixRelay");
    for path in [unknown_version, json_only, truncated] {
        assert!(
            engine
                .workspace_operation(request(
                    &engine,
                    WorkspaceAction::Open {
                        path: path.to_string_lossy().into_owned()
                    }
                ))
                .is_err()
        );
        assert_eq!(engine.read_catalog().unwrap(), before);
        assert_eq!(
            dir.files.bytes(&dir.join("graphs.json")).unwrap(),
            graph_bytes
        );
    }
    assert!(file_path("relative.mix").is_err());
    assert!(file_path("/fixture/wrong.json").is_err());
}
