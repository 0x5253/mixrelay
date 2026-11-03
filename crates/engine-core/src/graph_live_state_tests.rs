use super::*;
use crate::processor_ports::ProcessorStateError;
use mixrelay_control_protocol::GraphProcessorParameterChange;

fn change(id: u32, value: f64) -> GraphProcessorParameterChange {
    GraphProcessorParameterChange {
        node_id: "effect".into(),
        type_id: "fixture:parameters".into(),
        parameter_id: id,
        value,
    }
}

#[test]
fn recreated_provider_rejects_late_parameter_commands_without_mutating_new_instance() {
    use mixrelay_control_protocol::ProcessorParameterUpdate;
    let fixture = Fixture::new();
    let (engine, _) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let old_revision = engine.exchange.revisions().submitted;
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    let deleted = GraphDocument::new(old_revision + 1, vec![], vec![]);
    engine.apply_graph_request(deleted.clone().into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(
        engine
            .set_processor_parameter(
                "effect",
                7,
                ProcessorParameterUpdate {
                    expected_revision: deleted.revision,
                    value: 0.1,
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NodeNotFound
    );
    assert_eq!(engine.graph(), deleted);
    let mut replaced = parameter_document();
    replaced.revision = deleted.revision + 1;
    if let NodeConfig::Provider(provider) = &mut replaced.nodes[0].config {
        provider.parameters = serde_json::json!({"nativeToken":"NEW-INSTANCE"});
    }
    engine.apply_graph_request(replaced.clone().into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(
        engine
            .set_processor_parameter(
                "effect",
                7,
                ProcessorParameterUpdate {
                    expected_revision: old_revision,
                    value: 0.1,
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(engine.graph(), replaced);
    let mut disabled = replaced;
    disabled.revision += 1;
    disabled.nodes[0].control.enabled = false;
    let candidate = engine.catalog_candidate(&disabled);
    engine
        .publish_graph_locked(disabled.clone(), candidate)
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(
        engine
            .set_processor_parameter(
                "effect",
                7,
                ProcessorParameterUpdate {
                    expected_revision: disabled.revision,
                    value: 0.1,
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NodeDisabled
    );
    assert_eq!(engine.graph(), disabled);
    assert_eq!(engine.exchange.revisions().submitted, disabled.revision);
}

#[test]
fn disabled_node_preserves_saved_preset_without_worker_and_rejects_parameter_commands() {
    let mut document = parameter_document();
    document.nodes[0].control.enabled = false;
    if let NodeConfig::Provider(provider) = &mut document.nodes[0].config {
        provider.ports = Some(mixrelay_audio_graph::ProviderPorts::canonical_stereo(
            "fixture-stereo",
        ));
    }
    let (engine, _) = parameter_fixture(document);
    let mut proposed = engine.graph();
    proposed.revision += 1;
    let expected = provider_parameters(&proposed).clone();
    if let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config {
        provider.parameters = serde_json::json!({"nativeToken":"STALE"});
    }
    engine
        .preserve_live_processor_state_with(
            &mut proposed,
            &[],
            |_, _| panic!("disabled node cannot snapshot a worker"),
            |_, _| true,
        )
        .unwrap();
    assert_eq!(provider_parameters(&proposed), expected);
    assert_eq!(
        engine
            .preserve_live_processor_state_with(
                &mut proposed,
                &[change(7, 0.25)],
                |_, _| panic!("disabled node cannot snapshot a worker"),
                |_, _| true
            )
            .unwrap_err()
            .code,
        ErrorCode::NodeDisabled
    );
}

#[test]
fn disabling_failed_provider_retains_saved_preset_and_reports_warning() {
    let fixture = Fixture::new();
    let (engine, _) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let mut proposed = engine.graph();
    proposed.revision += 1;
    proposed.nodes[0].control.enabled = false;
    let expected = provider_parameters(&proposed).clone();
    engine
        .preserve_live_processor_state_with(
            &mut proposed,
            &[],
            |_, _| Err(ProcessorStateError::Timeout),
            |_, _| true,
        )
        .unwrap();
    assert_eq!(provider_parameters(&proposed), expected);
    assert!(engine.node_control_warning.lock().is_some());
    let candidate = engine.catalog_candidate(&proposed);
    let published = engine.publish_graph_locked(proposed, candidate).unwrap();
    assert!(published.persistence_error.is_some());
    assert!(engine.graph_catalog().unwrap().persistence_error.is_some());
    assert!(engine.catalog.warning().is_none());
    assert_eq!(provider_parameters(&engine.graph()), expected);
}

#[test]
fn unavailable_provider_restores_authoritative_preset_and_rejects_explicit_writes() {
    let mut document = parameter_document();
    if let NodeConfig::Provider(provider) = &mut document.nodes[0].config {
        provider.ports = Some(mixrelay_audio_graph::ProviderPorts::canonical_stereo(
            "fixture-stereo",
        ));
    }
    let (engine, _) = parameter_fixture(document);
    let mut proposed = engine.graph();
    proposed.revision += 1;
    let expected = provider_parameters(&proposed);
    if let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config {
        provider.parameters = serde_json::json!({"nativeToken":"STALE"});
    }
    engine
        .preserve_live_processor_state_with(
            &mut proposed,
            &[],
            |_, _| panic!("missing worker cannot be snapshotted"),
            |_, _| true,
        )
        .unwrap();
    assert_eq!(provider_parameters(&proposed), expected);
    let unchanged = proposed.clone();
    assert_eq!(
        engine
            .preserve_live_processor_state_with(
                &mut proposed,
                &[change(7, 0.25)],
                |_, _| panic!("missing worker cannot be snapshotted"),
                |_, _| true
            )
            .unwrap_err()
            .code,
        ErrorCode::ProcessorUnavailable
    );
    assert_eq!(proposed, unchanged);
}

#[test]
fn powered_off_provider_preserves_preset_and_accepts_explicit_parameter_delta() {
    let mut document = parameter_document();
    if let NodeConfig::Provider(provider) = &mut document.nodes[0].config {
        provider.ports = Some(mixrelay_audio_graph::ProviderPorts::canonical_stereo(
            "fixture-stereo",
        ));
    }
    let (engine, _) = parameter_fixture(document);
    engine.power.set_enabled(false);
    let mut proposed = engine.graph();
    proposed.revision += 1;
    let expected = provider_parameters(&proposed).clone();
    if let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config {
        provider.parameters = serde_json::json!({"nativeToken":"STALE"});
    }
    engine
        .preserve_live_processor_state_with(
            &mut proposed,
            &[change(7, 0.25)],
            |_, _| panic!("off provider must not snapshot a worker"),
            |_, _| true,
        )
        .unwrap();
    assert_eq!(
        provider_parameters(&proposed)["nativeToken"],
        expected["nativeToken"]
    );
    assert_eq!(provider_parameters(&proposed)["parameterValues"]["7"], 0.25);
}

#[test]
fn pending_startup_preserves_desired_provider_state_without_snapshotting_missing_worker() {
    let (engine, _) = parameter_fixture(parameter_document());
    engine.defer_catalog_restore(ProtocolError {
        code: ErrorCode::AudioDeviceDisconnected,
        parameters: serde_json::json!({"reason": "injected startup timeout"}),
    });
    let mut proposed = engine.graph();
    proposed.revision += 1;
    let original = proposed.clone();
    let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config else {
        panic!("fixture must contain the saved provider");
    };
    provider.parameters = serde_json::json!({"nativeToken":"STALE"});
    engine
        .preserve_live_processor_state_with(
            &mut proposed,
            &[],
            |_, _| panic!("an unstarted worker must not be snapshotted"),
            |_, _| true,
        )
        .unwrap();
    assert_eq!(proposed, original);
    let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config else {
        panic!("fixture must contain the saved provider");
    };
    provider.parameters = serde_json::json!({"nativeToken":"STALE"});
    engine
        .preserve_live_processor_state_with(
            &mut proposed,
            &[change(7, 0.25)],
            |_, _| panic!("an unstarted worker must not be snapshotted"),
            |_, _| true,
        )
        .unwrap();
    assert_eq!(provider_parameters(&proposed)["parameterValues"]["7"], 0.25);
    let mut expected = original;
    let NodeConfig::Provider(provider) = &mut expected.nodes[0].config else {
        panic!("fixture must contain the saved provider");
    };
    crate::persist_processor_parameter(provider, 7, 0.25);
    assert_eq!(proposed, expected);
}

#[test]
fn structural_edits_preserve_native_preset_and_only_explicit_undo_deltas_overlay_it() {
    let (engine, _) = parameter_fixture(parameter_document());
    for changes in [vec![], vec![change(7, 0.25)], vec![change(7, 0.75)]] {
        let mut proposed = engine.graph();
        proposed.revision += 1;
        if let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config {
            provider.parameters = serde_json::json!({
                "nativeToken":"STALE", "parameterValues":{"7":0.1,"9":0.3}
            });
        }
        let mutation = engine.mutation.lock();
        engine
            .preserve_live_processor_state_with(
                &mut proposed,
                &changes,
                |_, _| Ok(serde_json::json!({"nativeToken":"Ag=="})),
                |_, _| true,
            )
            .unwrap();
        drop(mutation);
        let parameters = provider_parameters(&proposed);
        assert_eq!(parameters["nativeToken"], "Ag==");
        assert!(parameters["parameterValues"].get("9").is_none());
        if let Some(edit) = changes.first() {
            assert_eq!(parameters["parameterValues"]["7"], edit.value);
        } else {
            assert!(parameters.get("parameterValues").is_none());
        }
        assert_eq!(engine.graph(), parameter_document());
    }
}

#[test]
fn new_or_replaced_provider_uses_requested_initial_state_without_capturing_old_instance() {
    let (engine, _) = parameter_fixture(parameter_document());
    for replace_type in [false, true] {
        let mut proposed = engine.graph();
        proposed.revision += 1;
        if replace_type {
            let NodeConfig::Provider(provider) = &mut proposed.nodes[0].config else {
                panic!()
            };
            provider.type_id = "fixture:new".into();
        } else {
            proposed.nodes[0].id = NodeId::from("new");
        }
        let original = proposed.clone();
        let mutation = engine.mutation.lock();
        engine
            .preserve_live_processor_state_with(
                &mut proposed,
                &[],
                |_, _| panic!("new instance captured"),
                |_, _| true,
            )
            .unwrap();
        drop(mutation);
        assert_eq!(proposed, original);
    }
}

#[test]
fn failed_second_snapshot_does_not_partially_rewrite_proposed_graph() {
    let mut document = parameter_document();
    let mut second = document.nodes[0].clone();
    second.id = NodeId::from("second");
    document.nodes.push(second);
    let (engine, _) = parameter_fixture(document);
    let mut proposed = engine.graph();
    proposed.revision += 1;
    let original = proposed.clone();
    let mutation = engine.mutation.lock();
    assert!(
        engine
            .preserve_live_processor_state_with(
                &mut proposed,
                &[],
                |id, _| {
                    if *id
                        == crate::graph_composition::runtime_node_id(
                            "session",
                            &NodeId::from("effect"),
                        )
                    {
                        Ok(serde_json::json!({"nativeToken":"Ag=="}))
                    } else {
                        Err(ProcessorStateError::Timeout)
                    }
                },
                |_, _| true
            )
            .is_err()
    );
    drop(mutation);
    assert_eq!(proposed, original);
}

#[test]
fn invalid_explicit_deltas_are_rejected_before_snapshot() {
    let (engine, _) = parameter_fixture(parameter_document());
    let mut missing = change(7, 0.5);
    missing.node_id = "missing".into();
    let mut changed_type = change(7, 0.5);
    changed_type.type_id = "fixture:other".into();
    for changes in [
        vec![change(7, f64::NAN)],
        vec![change(7, -1.0)],
        vec![change(7, 2.0)],
        vec![change(7, 0.2), change(7, 0.3)],
        vec![missing],
        vec![changed_type],
        vec![change(7, 0.5); 4097],
    ] {
        let mut proposed = engine.graph();
        proposed.revision += 1;
        let original = proposed.clone();
        let mutation = engine.mutation.lock();
        assert!(
            engine
                .preserve_live_processor_state_with(
                    &mut proposed,
                    &changes,
                    |_, _| panic!("invalid edit captured"),
                    |_, _| true
                )
                .is_err()
        );
        drop(mutation);
        assert_eq!(proposed, original);
    }
    let mut proposed = engine.graph();
    proposed.revision += 1;
    let mutation = engine.mutation.lock();
    assert!(
        engine
            .preserve_live_processor_state_with(
                &mut proposed,
                &[change(7, 0.5)],
                |_, _| panic!("readonly edit captured"),
                |_, _| false
            )
            .is_err()
    );
    drop(mutation);
}

#[test]
fn stale_or_pending_publication_never_snapshots_an_unconfirmed_instance() {
    let (engine, _) = parameter_fixture(parameter_document());
    for revision in [0, 1] {
        let mut proposed = engine.graph();
        proposed.revision = revision;
        let mutation = engine.mutation.lock();
        assert_eq!(
            engine
                .preserve_live_processor_state_with(
                    &mut proposed,
                    &[],
                    |_, _| panic!("stale capture"),
                    |_, _| true
                )
                .unwrap_err()
                .code,
            ErrorCode::GraphRevisionStale
        );
        drop(mutation);
    }
    let mut published = engine.graph();
    published.revision += 1;
    engine.apply_graph_locked(published.clone(), None).unwrap();
    published.revision += 1;
    let mutation = engine.mutation.lock();
    assert!(
        engine
            .preserve_live_processor_state_with(
                &mut published,
                &[],
                |_, _| panic!("pending capture"),
                |_, _| true
            )
            .is_err()
    );
    drop(mutation);
}

#[test]
fn reselecting_graph_does_not_snapshot_or_prepare_but_explicit_checkpoint_still_rejects_unavailable_worker()
 {
    let fixture = Fixture::new();
    let (engine, prepares) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let original = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let request = request(
        &engine,
        GraphCatalogAction::Select {
            id: "graph-1".into(),
        },
    );
    let revision = engine.exchange.revisions();
    let before_prepares = prepares.load(Ordering::Relaxed);
    engine.mutate_catalog(request).unwrap();
    assert_eq!(engine.exchange.revisions(), revision);
    assert_eq!(prepares.load(Ordering::Relaxed), before_prepares);
    assert!(engine.checkpoint_processor_state(None).is_err());
    assert_eq!(engine.graph(), parameter_document());
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        original
    );
    assert!(engine.graph_catalog().unwrap().persistence_error.is_some());
}
