use super::*;
use crate::processor_ports::ProcessorStateError;

#[test]
fn thirty_two_provider_checkpoint_preserves_each_state_with_independent_budgets() {
    let fixture = Fixture::new();
    let mut document = parameter_document();
    let template = document.nodes[0].clone();
    document.nodes = (0..32)
        .map(|index| {
            let mut node = template.clone();
            node.id = NodeId::new(format!("effect-{index}"));
            node
        })
        .collect();
    let (engine, prepares) = parameter_fixture(document);
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let mut captures = 0;
    engine
        .checkpoint_processor_state_with(None, |id, timeout| {
            // Exact full budgets catch accidental graph-wide deadline sharing without sleeps.
            assert_eq!(timeout, crate::processor_ports::PROCESSOR_STATE_TIMEOUT);
            captures += 1;
            Ok(serde_json::json!({"nativeToken": id.as_str()}))
        })
        .unwrap();
    assert_eq!(captures, 32);
    assert_eq!(prepares.load(Ordering::Relaxed), 0);
    let saved = engine.graph();
    for node in &saved.nodes {
        let NodeConfig::Provider(provider) = &node.config else {
            panic!("provider fixture")
        };
        assert_eq!(
            provider.parameters["nativeToken"],
            crate::graph_composition::runtime_node_id("graph-1", &node.id).as_str()
        );
    }
    drop(engine);
    let (restarted, _) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let restarted = restarted
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let restored = restarted.graph();
    assert!(restored.revision > saved.revision);
    assert_eq!(restored.nodes.len(), saved.nodes.len());
    for (actual, expected) in restored.nodes.iter().zip(&saved.nodes) {
        assert_eq!(actual, expected);
    }
    assert_eq!(restored.edges, saved.edges);
}

#[test]
fn native_editor_checkpoint_replaces_stale_overlays_and_survives_restart_without_audio_prepare() {
    let fixture = Fixture::new();
    let (engine, prepares) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    engine
        .apply_processor_parameter_update("effect", 7, 0.25, |_, _| Ok(()))
        .unwrap();
    let before = engine.graph_catalog().unwrap();
    let native = serde_json::json!({"nativeToken": "AgM="});
    let mutation = engine.mutation.lock();
    engine
        .checkpoint_processor_state_with(Some("effect"), |id, timeout| {
            assert_eq!(
                *id,
                crate::graph_composition::runtime_node_id("graph-1", &NodeId::from("effect"))
            );
            assert!(!timeout.is_zero() && timeout <= std::time::Duration::from_secs(5));
            Ok(native.clone())
        })
        .unwrap();
    drop(mutation);
    assert_eq!(provider_parameters(&engine.graph()), native);
    assert_eq!(prepares.load(Ordering::Relaxed), 0);
    let after = engine.graph_catalog().unwrap();
    assert_eq!(after.catalog_revision, before.catalog_revision);
    assert_eq!(after.document.revision, before.document.revision);
    assert!(after.persistence_error.is_none());
    drop(engine);
    let (restarted, _) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let restarted = restarted
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(provider_parameters(&restarted.graph()), native);
}

#[test]
fn native_editor_snapshot_failure_preserves_every_provider_and_existing_disk() {
    let fixture = Fixture::new();
    let mut document = parameter_document();
    let mut second = document.nodes[0].clone();
    second.id = NodeId::from("second");
    document.nodes.push(second);
    let (engine, _) = parameter_fixture(document.clone());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let disk = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    for failure in [
        ProcessorStateError::Unsupported,
        ProcessorStateError::Timeout,
        ProcessorStateError::NativeFailure,
        ProcessorStateError::Unavailable,
    ] {
        let mutation = engine.mutation.lock();
        let error = engine
            .checkpoint_processor_state_with(None, |id, _| {
                if *id
                    == crate::graph_composition::runtime_node_id("graph-1", &NodeId::from("effect"))
                {
                    Ok(serde_json::json!({"nativeToken": "Ag=="}))
                } else {
                    Err(failure)
                }
            })
            .unwrap_err();
        drop(mutation);
        assert_eq!(error.code, ErrorCode::ControlUnavailable);
        assert_eq!(engine.graph(), document);
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            disk
        );
        assert!(engine.graph_catalog().unwrap().persistence_error.is_some());
    }
}

fn two_graph_providers() -> (Fixture, EngineGraphController, Arc<AtomicU64>) {
    let fixture = Fixture::new();
    let document = parameter_document();
    fixture.0.files.insert(fixture.path(), serde_json::to_vec(&serde_json::json!({
            "version":1, "revision":1, "activeId":"alpha", "graphs":[
                {"id":"alpha", "name":"Alpha", "document":document,"layoutRevision":0,"layout":{"positions":{}}},
                {"id":"beta", "name":"Beta", "document":document,"layoutRevision":0,"layout":{"positions":{}}},
            ]
        }))
        .unwrap());
    let (engine, prepares) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    settle(&engine);
    (fixture, engine, prepares)
}

fn three_graph_providers() -> (Fixture, EngineGraphController) {
    let fixture = Fixture::new();
    let document = parameter_document();
    fixture.0.files.insert(fixture.path(), serde_json::to_vec(&serde_json::json!({
            "version":1, "revision":1, "activeId":"alpha", "graphs":[
                {"id":"alpha", "name":"Alpha", "document":document,"layoutRevision":0,"layout":{"positions":{}}},
                {"id":"beta", "name":"Beta", "document":document,"layoutRevision":0,"layout":{"positions":{}}},
                {"id":"gamma", "name":"Gamma", "document":document,"layoutRevision":0,"layout":{"positions":{}}},
            ]
        }))
        .unwrap());
    let (engine, _) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    settle(&engine);
    (fixture, engine)
}

#[test]
fn all_graph_checkpoint_uses_distinct_runtime_ids_and_keeps_authored_identity_without_prepare() {
    let (_fixture, engine, prepares) = two_graph_providers();
    let before_prepares = prepares.load(Ordering::Relaxed);
    let before_revision = engine.exchange.revisions();
    let mut captured = Vec::new();
    let mutation = engine.mutation.lock();
    engine
        .checkpoint_processor_state_with(None, |id, timeout| {
            assert_eq!(timeout, crate::processor_ports::PROCESSOR_STATE_TIMEOUT);
            captured.push(id.clone());
            Ok(serde_json::json!({"nativeToken":id.as_str()}))
        })
        .unwrap();
    drop(mutation);
    let catalog = engine.catalog_candidate(&engine.graph()).unwrap();
    for (graph_id, document) in catalog.documents() {
        let expected = crate::graph_composition::runtime_node_id(graph_id, &NodeId::from("effect"));
        assert!(captured.contains(&expected));
        assert_eq!(document.nodes[0].id.as_str(), "effect");
        assert_eq!(
            provider_parameters(document)["nativeToken"],
            expected.as_str()
        );
    }
    assert_eq!(captured.len(), 2);
    assert_eq!(prepares.load(Ordering::Relaxed), before_prepares);
    assert_eq!(engine.exchange.revisions(), before_revision);
}

#[test]
fn background_editor_checkpoint_targets_its_owner_and_failed_all_graph_export_is_atomic() {
    let (fixture, engine, _) = two_graph_providers();
    let selected = engine.graph();
    let mutation = engine.mutation.lock();
    engine
        .checkpoint_graph_processor_state_with("beta", "effect", |id, _| {
            assert_eq!(
                *id,
                crate::graph_composition::runtime_node_id("beta", &NodeId::from("effect"))
            );
            Ok(serde_json::json!({"nativeToken":"background"}))
        })
        .unwrap();
    drop(mutation);
    assert_eq!(engine.graph(), selected);
    assert_eq!(
        provider_parameters(&engine.read_catalog_document("beta").unwrap())["nativeToken"],
        "background"
    );
    let disk = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let mutation = engine.mutation.lock();
    let result = engine.checkpoint_processor_state_with(None, |id, _| {
        if *id == crate::graph_composition::runtime_node_id("alpha", &NodeId::from("effect")) {
            Ok(serde_json::json!({"nativeToken":"must-not-persist"}))
        } else {
            Err(ProcessorStateError::NativeFailure)
        }
    });
    drop(mutation);
    assert!(result.is_err());
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        disk
    );
    assert_eq!(engine.graph(), selected);
}

#[test]
fn composition_rebuild_preserves_background_state_without_replacing_selected_edit_deltas() {
    let (_fixture, engine, prepares) = two_graph_providers();
    let before_prepares = prepares.load(Ordering::Relaxed);
    let mut selected = engine.graph();
    let NodeConfig::Provider(provider) = &mut selected.nodes[0].config else {
        panic!("fixture")
    };
    provider.parameters =
        serde_json::json!({"nativeToken":"selected", "parameterValues":{"7":0.4}});
    let mut candidate = engine.catalog_candidate(&selected).unwrap();
    let mutation = engine.mutation.lock();
    engine
        .preserve_catalog_provider_state_with_exclusion(
            &mut candidate,
            |id, _| {
                assert_eq!(
                    *id,
                    crate::graph_composition::runtime_node_id("beta", &NodeId::from("effect"))
                );
                Ok(serde_json::json!({"nativeToken":"background-live"}))
            },
            None,
        )
        .unwrap();
    drop(mutation);
    assert_eq!(*candidate.active_document(), selected);
    let (_, background) = candidate.documents().find(|(id, _)| *id == "beta").unwrap();
    assert_eq!(
        provider_parameters(background)["nativeToken"],
        "background-live"
    );
    assert_eq!(prepares.load(Ordering::Relaxed), before_prepares);
}

#[test]
fn retry_state_capture_skips_only_the_failed_instance_and_keeps_its_saved_preset() {
    let (_fixture, engine) = three_graph_providers();
    let initial_failed_state = provider_parameters(&engine.read_catalog_document("beta").unwrap());
    let failed_id = crate::graph_composition::runtime_node_id("beta", &NodeId::from("effect"));
    let mut candidate = engine.catalog_candidate(&engine.graph()).unwrap();
    let mut selected = engine.graph();
    selected.revision = engine.exchange.revisions().submitted + 1;
    let mut captured = Vec::new();
    let mutation = engine.mutation.lock();
    engine
        .preserve_live_processor_state_for(
            &mut selected,
            crate::graph_catalog::live_state::LiveStateIntent::Retry(&failed_id),
            |id, _| {
                captured.push(id.clone());
                Ok(serde_json::json!({"nativeToken":"selected-live"}))
            },
            |_, _| true,
        )
        .unwrap();
    candidate.update_document(selected.clone());
    engine
        .preserve_catalog_provider_state_with_exclusion(
            &mut candidate,
            |id, _| {
                captured.push(id.clone());
                Ok(serde_json::json!({"nativeToken":"healthy-live"}))
            },
            Some(&failed_id),
        )
        .unwrap();
    drop(mutation);

    assert_eq!(
        captured,
        vec![
            crate::graph_composition::runtime_node_id("alpha", &NodeId::from("effect")),
            crate::graph_composition::runtime_node_id("gamma", &NodeId::from("effect")),
        ]
    );
    assert_eq!(
        provider_parameters(&selected)["nativeToken"],
        "selected-live"
    );
    let (_, failed) = candidate.documents().find(|(id, _)| *id == "beta").unwrap();
    assert_eq!(provider_parameters(failed), initial_failed_state);
    let (_, healthy) = candidate
        .documents()
        .find(|(id, _)| *id == "gamma")
        .unwrap();
    assert_eq!(provider_parameters(healthy)["nativeToken"], "healthy-live");
}

#[test]
fn native_editor_checkpoint_rejects_missing_nodes_and_pending_graph_before_snapshot() {
    let fixture = Fixture::new();
    let (engine, _) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(
        engine
            .checkpoint_processor_state(Some("missing"))
            .unwrap_err()
            .code,
        ErrorCode::GraphInvalid
    );
    let mut document = engine.graph();
    document.revision += 1;
    engine
        .apply_graph_locked(document.clone(), engine.catalog_candidate(&document))
        .unwrap();
    let disk = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let mutation = engine.mutation.lock();
    assert!(
        engine
            .checkpoint_processor_state_with(None, |_, _| panic!("pending graph captured"))
            .is_err()
    );
    drop(mutation);
    assert_eq!(engine.graph(), document);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        disk
    );
}

#[test]
fn native_editor_checkpoint_storage_failure_rolls_back_and_same_revision_retry_succeeds() {
    for commit_failure in [false, true] {
        let fixture = Fixture::new();
        let document = parameter_document();
        let (engine, _) = parameter_fixture(document.clone());
        let engine = engine
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        let disk = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
        if commit_failure {
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
        let native = serde_json::json!({"nativeToken": "BA=="});
        let mutation = engine.mutation.lock();
        assert!(
            engine
                .checkpoint_processor_state_with(Some("effect"), |_, _| Ok(native.clone()))
                .is_err()
        );
        drop(mutation);
        assert_eq!(engine.graph(), document);
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            disk
        );
        assert!(engine.graph_catalog().unwrap().persistence_error.is_some());
        fixture.0.files.allow_stage(&fixture.path());
        fixture.0.files.allow_commit(&fixture.path());
        let mutation = engine.mutation.lock();
        engine
            .checkpoint_processor_state_with(Some("effect"), |_, _| Ok(native.clone()))
            .unwrap();
        drop(mutation);
        assert_eq!(engine.graph().revision, document.revision);
        assert_eq!(provider_parameters(&engine.graph()), native);
        assert!(engine.graph_catalog().unwrap().persistence_error.is_none());
        assert_ne!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            disk
        );
    }
}
