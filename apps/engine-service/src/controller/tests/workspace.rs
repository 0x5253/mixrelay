//! Native file replacement and portable state-asset contracts.
use std::{fs::File, path::Path};

use super::*;
use mixrelay_audio_graph::{NodeId, NodeSpec, ProviderNodeConfig, ProviderPorts};
use mixrelay_control_protocol::{WorkspaceAction, WorkspaceRequest};
use mixrelay_control_server::ControlBackend;
use mixrelay_processor_state::{StateRef, StateStore, read_bundle, write_bundle};
use sha2::{Digest, Sha256};

fn request(engine: &EngineGraphController, operation: WorkspaceAction) -> WorkspaceRequest {
    let catalog = engine.graph_catalog().unwrap();
    WorkspaceRequest {
        file_digest: None,
        expected_revision: catalog.document.revision,
        expected_catalog_revision: catalog.catalog_revision,
        operation,
    }
}

fn write_bundle_fixture(
    path: &Path,
    catalog: &serde_json::Value,
    references: &[StateRef],
    store: &StateStore,
    version: u32,
) {
    let manifest = serde_json::to_vec(
        &serde_json::json!({"format":"MixRelay", "version":version, "catalog":catalog}),
    )
    .unwrap();
    let mut file = File::create(path).unwrap();
    write_bundle(&mut file, &manifest, references, store).unwrap();
}

fn add_provider(catalog: &mut serde_json::Value, id: &str, parameters: serde_json::Value) {
    let mut ports = ProviderPorts::canonical_stereo("fixture-eight");
    for bus in ports.inputs.iter_mut().chain(&mut ports.outputs) {
        bus.channels = 8;
        bus.channel_format = mixrelay_audio_graph::ProviderChannelFormat::Surround7_1;
    }
    let node = NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from(id),
        label: Some("Saved Effect".into()),
        config: NodeConfig::Provider(ProviderNodeConfig {
            type_id: "vst3:not-installed".into(),
            parameters,
            ports: Some(ports),
        }),
    };
    catalog["graphs"][0]["document"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(node).unwrap());
    catalog["graphs"][0]["layout"]["positions"][id] = serde_json::json!({"x":150.0,"y":220.0});
}

fn with_state(reference: &StateRef) -> serde_json::Value {
    serde_json::json!({
        "stateRef": reference,
        "parameterValues": {"7": 0.75}
    })
}

#[test]
fn failed_save_does_not_destroy_destination_or_current_catalog() {
    let dir = mixrelay_test_support::FixtureDir::new("workspace-write").unwrap();
    let engine = controller(GraphDocument::new(9, vec![], vec![]))
        .with_graph_catalog_path(dir.join("graphs.json"));
    let path = dir.join("directory.mix");
    std::fs::create_dir(&path).unwrap();
    let before = engine.graph_catalog().unwrap();
    assert!(
        engine
            .workspace_file(request(
                &engine,
                WorkspaceAction::Save {
                    path: path.to_string_lossy().into_owned()
                }
            ))
            .is_err()
    );
    assert!(path.is_dir());
    assert_eq!(before, engine.graph_catalog().unwrap());
}

#[test]
fn preview_hashes_every_bundle_byte_without_importing_state_and_fences_open() {
    let dir = mixrelay_test_support::FixtureDir::new("workspace-preview").unwrap();
    let engine = controller(GraphDocument::new(1, vec![], vec![]))
        .with_graph_catalog_path(dir.join("graphs.json"));
    let path = dir.join("preview.mix").to_string_lossy().into_owned();
    engine
        .workspace_file(request(
            &engine,
            WorkspaceAction::Save { path: path.clone() },
        ))
        .unwrap();
    let before = engine.graph_catalog().unwrap();
    let mut open = request(&engine, WorkspaceAction::Open { path: path.clone() });
    let preview = engine.preview_workspace_file(open.clone()).unwrap();
    let file_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        preview.file_digest,
        Some(format!("{:x}", Sha256::digest(&file_bytes)))
    );
    assert_eq!(engine.graph_catalog().unwrap(), before);
    assert!(preview.manifest.sources.is_empty());
    assert!(preview.manifest.revision > before.document.revision);

    let changed = catalog_fixture(GraphDocument::new(12, vec![], vec![]));
    let store = StateStore::new(dir.join("fixture-assets"));
    write_bundle_fixture(Path::new(&path), &changed, &[], &store, 1);
    open.file_digest = preview.file_digest;
    assert!(engine.workspace_file(open).is_err());
    assert_eq!(engine.graph_catalog().unwrap(), before);

    let mut retry = request(&engine, WorkspaceAction::Open { path });
    let preview = engine.preview_workspace_file(retry.clone()).unwrap();
    retry.file_digest = preview.file_digest;
    let accepted = engine.workspace_file(retry).unwrap();
    assert_eq!(accepted.document.revision, preview.manifest.revision);
}

#[test]
fn missing_unreferenced_and_malformed_state_assets_do_not_change_catalog() {
    let dir = mixrelay_test_support::FixtureDir::new("workspace-assets-invalid").unwrap();
    let engine = controller(GraphDocument::new(1, vec![], vec![]))
        .with_graph_catalog_path(dir.join("graphs.json"));
    let before = engine.graph_catalog().unwrap();
    let store = engine.processor_files.snapshot().unwrap();
    let reference = store.put(b"fixture processor state").unwrap();

    let mut missing_catalog = catalog_fixture(GraphDocument::new(2, vec![], vec![]));
    add_provider(
        &mut missing_catalog,
        "missing-state",
        with_state(&reference),
    );
    let missing = dir.join("missing-asset.mix");
    write_bundle_fixture(&missing, &missing_catalog, &[], &store, 1);

    let corrupted = dir.join("corrupted-payload.mix");
    write_bundle_fixture(
        &corrupted,
        &missing_catalog,
        std::slice::from_ref(&reference),
        &store,
        1,
    );
    let mut corrupted_bytes = std::fs::read(&corrupted).unwrap();
    let final_byte = corrupted_bytes.len() - 1;
    corrupted_bytes[final_byte] ^= 0xff;
    std::fs::write(&corrupted, corrupted_bytes).unwrap();

    let unreferenced = dir.join("unreferenced-asset.mix");
    write_bundle_fixture(
        &unreferenced,
        &catalog_fixture(GraphDocument::new(3, vec![], vec![])),
        std::slice::from_ref(&reference),
        &store,
        1,
    );

    let mut malformed_catalog = catalog_fixture(GraphDocument::new(4, vec![], vec![]));
    add_provider(
        &mut malformed_catalog,
        "malformed-state",
        serde_json::json!({"stateRef":{"sha256":"bad","byteLength":7}}),
    );
    let malformed = dir.join("malformed-state.mix");
    write_bundle_fixture(&malformed, &malformed_catalog, &[], &store, 1);

    for path in [missing, unreferenced, malformed, corrupted] {
        assert!(
            engine
                .workspace_file(request(
                    &engine,
                    WorkspaceAction::Open {
                        path: path.to_string_lossy().into_owned()
                    }
                ))
                .is_err()
        );
        assert_eq!(engine.graph_catalog().unwrap(), before);
    }
}

#[test]
fn portable_bundle_reopens_large_states_after_source_store_is_deleted() {
    const STATE_BYTES: usize = 9 * 1024 * 1024;
    let dir = mixrelay_test_support::FixtureDir::new("workspace-portable-state").unwrap();
    let driver_inventory = default_duplex_document(1, None);
    let engine =
        controller_with_inventory(GraphDocument::new(1, vec![], vec![]), &driver_inventory)
            .with_graph_catalog_path(dir.join("graphs.json"));
    let source_store = engine.processor_files.snapshot().unwrap();
    let first_state = vec![0x35; STATE_BYTES];
    let second_state = vec![0xa7; STATE_BYTES];
    let first_ref = source_store.put(&first_state).unwrap();
    let second_ref = source_store.put(&second_state).unwrap();
    let mut catalog = catalog_fixture(default_duplex_document(1, Some(-6.0)));
    add_provider(&mut catalog, "missing-effect-a", with_state(&first_ref));
    add_provider(&mut catalog, "missing-effect-b", with_state(&second_ref));
    let references = state_references(&catalog);
    assert!(
        references
            .windows(2)
            .all(|pair| pair[0].sha256() < pair[1].sha256())
    );
    let source = dir.join("portable-source.mix");
    write_bundle_fixture(&source, &catalog, &references, &source_store, 1);
    let source_bytes = std::fs::read(&source).unwrap();
    assert!(source_bytes.len() > 16 * 1024 * 1024);
    let preview_engine =
        controller_with_inventory(GraphDocument::new(1, vec![], vec![]), &driver_inventory)
            .with_graph_catalog_path(dir.join("preview-graphs.json"));
    let preview_store = preview_engine.processor_files.snapshot().unwrap();
    let preview_request = request(
        &preview_engine,
        WorkspaceAction::Open {
            path: source.to_string_lossy().into_owned(),
        },
    );
    let preview = preview_engine
        .preview_workspace_file(preview_request)
        .unwrap();
    assert!(!preview_store.root().exists());
    assert_eq!(
        preview.file_digest,
        Some(format!("{:x}", Sha256::digest(&source_bytes)))
    );
    drop(source_bytes);

    let mut open = request(
        &engine,
        WorkspaceAction::Open {
            path: source.to_string_lossy().into_owned(),
        },
    );

    std::fs::remove_dir_all(source_store.root()).unwrap();
    open.file_digest = preview.file_digest;
    engine.workspace_file(open).unwrap();
    let _processor = settle(&engine);

    let imported_store = engine.processor_files.snapshot().unwrap();
    assert_eq!(imported_store.read(&first_ref).unwrap(), first_state);
    assert_eq!(imported_store.read(&second_ref).unwrap(), second_state);
    let output = dir.join("portable-output.mix");
    engine
        .workspace_file(request(
            &engine,
            WorkspaceAction::Save {
                path: output.to_string_lossy().into_owned(),
            },
        ))
        .unwrap();

    let reopened_store = StateStore::new(dir.join("reopened-assets"));
    let (manifest, bundled_refs) =
        read_bundle(File::open(&output).unwrap(), &reopened_store).unwrap();
    let reopened: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(state_references(&reopened["catalog"]), bundled_refs);
    assert_eq!(reopened_store.read(&first_ref).unwrap(), first_state);
    assert_eq!(reopened_store.read(&second_ref).unwrap(), second_state);
    assert_saved_provider_overlays(&reopened["catalog"]);
}

fn assert_saved_provider_overlays(catalog: &serde_json::Value) {
    for id in ["missing-effect-a", "missing-effect-b"] {
        let document: GraphDocument =
            serde_json::from_value(catalog["graphs"][0]["document"].clone()).unwrap();
        let provider = document
            .nodes
            .iter()
            .find(|node| node.id.as_str() == id)
            .unwrap();
        let NodeConfig::Provider(provider) = &provider.config else {
            panic!("fixture must remain a provider");
        };
        assert_eq!(provider.parameters["parameterValues"]["7"], 0.75);
    }
}

fn catalog_fixture(document: GraphDocument) -> serde_json::Value {
    let document = serde_json::to_value(document).unwrap();
    serde_json::json!({"version":1,"revision":1,"activeId":"graph-1","graphs":[{"id":"graph-1","name":"Untitled Graph","document":document,"layoutRevision":0,"layout":{"positions":{}}}]})
}
fn state_references(catalog: &serde_json::Value) -> Vec<StateRef> {
    let mut references: Vec<StateRef> = catalog["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|graph| graph["document"]["nodes"].as_array().unwrap())
        .filter_map(|node| node["config"]["parameters"].get("stateRef"))
        .map(|reference| serde_json::from_value(reference.clone()).unwrap())
        .collect();
    references.sort_by(|a, b| a.sha256().cmp(b.sha256()));
    references.dedup();
    references
}
