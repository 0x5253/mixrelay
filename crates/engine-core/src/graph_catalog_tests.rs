use super::*;
use crate::controller_test_support::settle;
use crate::endpoint_settings_tests::controller;
use mixrelay_audio_graph::{ExecutionPlan, GraphCompiler};
use mixrelay_control_protocol::{GraphCatalogAction, GraphCatalogRequest};
use mixrelay_render::BlockProcessor;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "graph_layout_tests.rs"]
mod layout;

#[path = "graph_native_state_tests.rs"]
mod native_state;

#[path = "graph_live_state_tests.rs"]
mod live_state;

mod publication;
mod removal;

struct Fixture(crate::test_support::Documents);

#[test]
fn durable_catalog_reads_do_not_wait_for_native_preparation() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(9, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let expected = engine.read_catalog().unwrap();
    let expected_layout = engine.read_graph_layout(&expected.active_id).unwrap();
    let response = std::thread::scope(|scope| {
        let mutation = engine.mutation.lock();
        let (send, receive) = std::sync::mpsc::channel();
        let id = &expected.active_id;
        let engine = &engine;
        scope.spawn(move || {
            let snapshots = (
                engine.read_catalog(),
                engine.read_catalog_document(id),
                engine.read_graph_layout(id),
            );
            let _ = send.send(snapshots);
        });
        let response = receive.recv_timeout(std::time::Duration::from_secs(1));
        // Release even on regression so the test joins its reader instead of hanging.
        drop(mutation);
        response
    });
    let (catalog, document, layout) = response.expect("reads must not wait for mutation");
    assert_eq!(catalog.unwrap(), expected);
    assert_eq!(document.unwrap(), expected.document);
    assert_eq!(layout.unwrap(), expected_layout);
}

impl Fixture {
    fn new() -> Self {
        Self(crate::test_support::Documents::new("catalog"))
    }
    fn path(&self) -> PathBuf {
        self.0.join("graphs.json")
    }
}
fn request(controller: &EngineGraphController, action: GraphCatalogAction) -> GraphCatalogRequest {
    GraphCatalogRequest {
        expected_revision: controller.graph().revision,
        expected_catalog_revision: controller.graph_catalog().unwrap().catalog_revision,
        action,
    }
}

/// A deterministic provider prepare stand-in: durable controller recall is real, but no
/// installed third-party binary or user authorization is required by these storage tests.
struct ParameterFixtureFactory(Arc<AtomicU64>);
struct ParameterFixtureResolver;

impl mixrelay_audio_graph::NodeResolver for ParameterFixtureResolver {
    fn resolve(
        &self,
        node: &mixrelay_audio_graph::NodeSpec,
        context: &CompileContext,
    ) -> Result<mixrelay_audio_graph::ResolvedNode, mixrelay_audio_graph::NodeResolutionError> {
        use mixrelay_audio_graph::{
            BuiltInNodeResolver, CompiledPort, PortDirection, PortId, ResolvedNode,
        };
        if matches!(&node.config, NodeConfig::Provider(provider) if provider.type_id.as_str() == "fixture:parameters")
        {
            return Ok(ResolvedNode {
                inputs: vec![CompiledPort::new(
                    PortId::from("audio"),
                    PortDirection::Input,
                    2,
                )],
                outputs: vec![CompiledPort::new(
                    PortId::from("audio"),
                    PortDirection::Output,
                    2,
                )],
            });
        }
        BuiltInNodeResolver.resolve(node, context)
    }
}

impl ProcessorFactory for ParameterFixtureFactory {
    fn compiler(&self) -> GraphCompiler {
        GraphCompiler::new(ParameterFixtureResolver)
    }

    fn prepare(
        &self,
        execution: &ExecutionPlan,
    ) -> Result<Box<BlockProcessor>, PrepareProcessorError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        // State/persistence tests do not run a vendor. The caller still compiles the real
        // provider topology with the fixture resolver before substituting silent DSP here.
        let empty = GraphCompiler::default()
            .compile(
                &GraphDocument::new(execution.revision(), vec![], vec![]),
                CompileContext {
                    format: execution.format(),
                    limits: CompileLimits::default(),
                },
            )
            .expect("empty fixture compiles with the already validated execution format");
        crate::test_support::DspFactory::new(default_peak_meter_bank()).prepare(&empty)
    }
}

fn parameter_fixture(document: GraphDocument) -> (EngineGraphController, Arc<AtomicU64>) {
    let mut engine = controller(document);
    let prepares = Arc::new(AtomicU64::new(0));
    engine.factory = Arc::new(ParameterFixtureFactory(Arc::clone(&prepares)));
    (engine, prepares)
}

fn parameter_document() -> GraphDocument {
    GraphDocument::new(
        1,
        vec![mixrelay_audio_graph::NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("effect"),
            label: None,
            config: NodeConfig::Provider(mixrelay_audio_graph::ProviderNodeConfig {
                ports: None,
                type_id: "fixture:parameters".into(),
                parameters: serde_json::json!({"nativeToken": "AQ=="}),
            }),
        }],
        vec![],
    )
}

fn provider_parameters(document: &GraphDocument) -> serde_json::Value {
    let NodeConfig::Provider(provider) = &document.nodes[0].config else {
        panic!("provider fixture")
    };
    provider.parameters.clone()
}

#[test]
fn accepted_processor_parameters_are_durable_without_repreparing_and_restore_on_restart() {
    let fixture = Fixture::new();
    let (engine, prepares) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let revision = engine.graph().revision;
    let publications = AtomicU64::new(0);
    std::thread::scope(|scope| {
        for id in [7, 9] {
            let engine = &engine;
            let publications = &publications;
            scope.spawn(move || {
                for tick in 1..=24 {
                    let value = f64::from(tick) / 32.0;
                    let response = engine
                        .apply_processor_parameter_update("effect", id, value, |_, _| {
                            publications.fetch_add(1, Ordering::Relaxed);
                            Ok(())
                        })
                        .unwrap();
                    assert!(response.persistence_error.is_none());
                }
            });
        }
    });
    assert_eq!(publications.load(Ordering::Relaxed), 48);
    assert_eq!(prepares.load(Ordering::Relaxed), 0);
    assert_eq!(engine.graph().revision, revision);
    assert_eq!(engine.exchange.revisions().submitted, revision);
    let expected = provider_parameters(&engine.graph());
    assert_eq!(
        expected["parameterValues"],
        serde_json::json!({"7": 0.75, "9": 0.75})
    );
    assert_eq!(expected["nativeToken"], "AQ==");
    drop(engine);
    let (restarted, prepares) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
    let restarted = restarted
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(
        prepares.load(Ordering::Relaxed),
        1,
        "only startup prepares the recalled processor"
    );
    assert_eq!(provider_parameters(&restarted.graph()), expected);
    assert!(
        restarted
            .graph_catalog()
            .unwrap()
            .persistence_error
            .is_none()
    );
}

#[test]
fn rejected_parameter_does_not_write_and_catalog_disabled_is_explicitly_in_memory() {
    let fixture = Fixture::new();
    let (engine, _) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let original = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    assert!(
        engine
            .apply_processor_parameter_update("effect", 7, 0.25, |_, _| {
                Err(processor_parameter_error(
                    "effect",
                    7,
                    "fixture worker unavailable",
                ))
            })
            .is_err()
    );
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        original
    );
    assert!(
        provider_parameters(&engine.graph())
            .get("parameterValues")
            .is_none()
    );
    let (memory_only, _) = parameter_fixture(parameter_document());
    assert!(memory_only.catalog.inspect(|value| value.is_none()));
    assert!(
        memory_only
            .apply_processor_parameter_update("effect", 7, 0.25, |_, _| Ok(()))
            .unwrap()
            .persistence_error
            .is_none()
    );
}

#[test]
fn independent_documents_survive_switch_gain_edits_rename_and_restart() {
    let fixture = Fixture::new();
    let first = controller(default_duplex_document(1, Some(-6.0)))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let original = first.graph();
    let created = first
        .mutate_graph_catalog(request(
            &first,
            GraphCatalogAction::Create {
                name: "SPAT 64".into(),
            },
        ))
        .unwrap();
    assert!(created.document.nodes.is_empty());
    assert_eq!(created.graphs.len(), 2);
    let second_id = created.active_id;
    let restored = first
        .mutate_graph_catalog(request(
            &first,
            GraphCatalogAction::Select {
                id: "graph-1".into(),
            },
        ))
        .unwrap();
    assert_eq!(restored.document.nodes, original.nodes);
    assert_eq!(restored.document.edges, original.edges);
    let mut edited = first.graph();
    edited.revision += 1;
    let gain = edited
        .nodes
        .iter_mut()
        .find_map(|node| match &mut node.config {
            NodeConfig::Gain(gain) => Some(gain),
            _ => None,
        })
        .unwrap();
    gain.gain_db = -12.0;
    first.apply_graph_request(edited.clone().into()).unwrap();
    settle(&first);
    first
        .mutate_graph_catalog(request(
            &first,
            GraphCatalogAction::Rename {
                id: "graph-1".into(),
                name: "Ableton ↔ SPAT".into(),
            },
        ))
        .unwrap();
    first
        .mutate_graph_catalog(request(
            &first,
            GraphCatalogAction::Select {
                id: second_id.clone(),
            },
        ))
        .unwrap();
    drop(first);
    let second = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(second.graph_catalog().unwrap().active_id, second_id);
    assert!(second.graph().nodes.is_empty());
    settle(&second);
    let restored = second
        .mutate_graph_catalog(request(
            &second,
            GraphCatalogAction::Select {
                id: "graph-1".into(),
            },
        ))
        .unwrap();
    assert_eq!(restored.graphs[0].name, "Ableton ↔ SPAT");
    assert_eq!(restored.document.nodes, edited.nodes);
    assert_eq!(restored.document.edges, edited.edges);
}

#[test]
fn restart_revision_exceeds_every_saved_graph_and_bootstrap_revision() {
    for (active, inactive, bootstrap, next) in [(97, 90, 1, 98), (3, 90, 1, 91), (3, 90, 200, 201)]
    {
        let fixture = Fixture::new();
        let document = default_duplex_document(active, Some(-6.0));
        let saved = serde_json::json!({
            "version": 1, "revision": 17, "activeId": "selected", "graphs": [
                {"id": "selected", "name": "Selected", "document": document,"layoutRevision":0,"layout":{"positions":{}}},
                {"id": "other", "name": "Other", "document": GraphDocument::new(inactive, vec![], vec![]),
                 "layoutRevision": 0, "layout": {"positions": {}}}
            ]
        });
        fixture
            .0
            .files
            .insert(fixture.path(), serde_json::to_vec(&saved).unwrap());
        let engine = controller(default_duplex_document(bootstrap, None))
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        assert_eq!(engine.graph().revision, next);
        assert_eq!(engine.graph().nodes, document.nodes);
        assert_eq!(engine.graph().edges, document.edges);
        settle(&engine);
        assert_eq!(
            engine
                .apply_graph_request(document.into())
                .unwrap_err()
                .code,
            ErrorCode::GraphRevisionStale
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted["graphs"][0]["document"]["revision"], next);
        assert_eq!(persisted["graphs"][1], saved["graphs"][1]);
    }
}

#[test]
fn exhausted_saved_revision_keeps_saved_document_readable_without_replacing_bytes() {
    let fixture = Fixture::new();
    let first = controller(default_duplex_document(u64::MAX, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let saved = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    drop(first);
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(engine.graph().revision, u64::MAX);
    assert_eq!(engine.graph_catalog().unwrap().document.revision, u64::MAX);
    assert!(engine.catalog_restore.is_pending());
    assert_eq!(engine.graph_status().submitted_revision, 1);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        saved
    );
}

#[test]
fn accepted_parameter_warns_for_corrupt_catalog_without_overwriting_its_bytes() {
    let fixture = Fixture::new();
    fixture
        .0
        .files
        .insert(fixture.path(), b"corrupt user catalog");
    let (engine, prepares) = parameter_fixture(parameter_document());
    let engine = engine
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let response = engine
        .apply_processor_parameter_update("effect", 7, 0.5, |_, _| Ok(()))
        .unwrap();
    assert!(response.persistence_error.unwrap().contains("unavailable"));
    assert_eq!(
        provider_parameters(&engine.graph())["parameterValues"]["7"],
        0.5
    );
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        b"corrupt user catalog"
    );
    assert_eq!(prepares.load(Ordering::Relaxed), 0);
}

#[test]
fn a_targeted_apply_is_rejected_after_another_graph_is_selected() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let first_id = engine.graph_catalog().unwrap().active_id;
    let targeted = |document: GraphDocument, id: &str| GraphApplyRequest {
        expected_graph_id: Some(id.into()),
        ..document.into()
    };
    // The selected graph accepts a document authored for it.
    let mut document = engine.graph();
    document.revision += 1;
    engine
        .apply_graph_request(targeted(document, &first_id))
        .unwrap();
    settle(&engine);
    let created = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Create {
                name: "Other".into(),
            },
        ))
        .unwrap();
    assert_ne!(created.active_id, first_id);
    let saved = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    // A document authored for the first graph never lands on the newly selected one.
    let mut stale = engine.graph();
    stale.revision += 1;
    stale.nodes = default_duplex_document(1, None).nodes;
    let error = engine
        .apply_graph_request(targeted(stale, &first_id))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::GraphRevisionStale);
    assert_eq!(
        error.parameters["selectedGraphId"],
        created.active_id.as_str()
    );
    assert!(engine.graph().nodes.is_empty());
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        saved
    );
}

#[test]
fn stale_writer_and_pending_publication_do_not_switch_or_change_saved_bytes() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let stale = request(&engine, GraphCatalogAction::Create { name: "old".into() });
    let mut document = engine.graph();
    document.revision += 1;
    engine.apply_graph_request(document.into()).unwrap();
    let saved = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    assert_eq!(
        engine.mutate_graph_catalog(stale).unwrap_err().code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(
        engine
            .mutate_graph_catalog(request(
                &engine,
                GraphCatalogAction::Create {
                    name: "pending".into()
                }
            ))
            .unwrap_err()
            .code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        saved
    );
    assert_eq!(engine.graph_catalog().unwrap().graphs.len(), 1);
}

#[test]
fn corrupt_catalog_is_preserved_and_fallback_remains_editable() {
    let fixture = Fixture::new();
    fixture.0.files.insert(fixture.path(), b"not valid JSON");
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert!(engine.graph_catalog().is_err());
    let mut document = engine.graph();
    document.revision += 1;
    engine.apply_graph_request(document.into()).unwrap();
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        b"not valid JSON"
    );
}

#[test]
fn unavailable_background_native_target_keeps_catalog_readable_and_preserves_bytes() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let mut saved: serde_json::Value = serde_json::from_slice(
        &crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
    )
    .unwrap();
    let mut invalid = saved["graphs"][0].clone();
    invalid["id"] = "unavailable".into();
    invalid["document"]["nodes"][0]["config"]["id"] = "unavailable-device".into();
    invalid["document"]["nodes"][0]["config"]["displayName"] = "rejected".into();
    saved["graphs"].as_array_mut().unwrap().push(invalid);
    fixture
        .0
        .files
        .insert(fixture.path(), serde_json::to_vec(&saved).unwrap());
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    drop(engine);
    let bootstrap = default_duplex_document(1, None);
    let engine = controller(bootstrap.clone())
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    assert_eq!(engine.graph_catalog().unwrap().graphs.len(), 2);
    assert_eq!(engine.graph().nodes, bootstrap.nodes);
    assert!(engine.catalog_restore.is_pending());
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        before
    );
}

#[test]
fn metadata_revision_rejects_concurrent_rename_without_changing_audio_revision() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let stale = request(
        &engine,
        GraphCatalogAction::Rename {
            id: "graph-1".into(),
            name: "stale".into(),
        },
    );
    let renamed = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Rename {
                id: "graph-1".into(),
                name: "current".into(),
            },
        ))
        .unwrap();
    assert_eq!(renamed.document.revision, 1);
    assert_eq!(
        engine.mutate_graph_catalog(stale).unwrap_err().code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(engine.graph_catalog().unwrap().graphs[0].name, "current");
}

#[test]
fn native_resource_failure_cancels_only_this_transactions_prepared_endpoint() {
    struct Endpoint {
        inner: ContractOnlyVirtualEndpointBackend,
        pending: std::sync::atomic::AtomicBool,
        cancellations: AtomicU64,
    }
    impl VirtualEndpointBackend for Endpoint {
        fn capabilities(&self) -> &VirtualEndpointCapabilities {
            self.inner.capabilities()
        }
        fn prepare_graph_attachments(
            &self,
            _: &[VirtualEndpointSpec],
        ) -> Result<(), HostAudioBackendError> {
            if self.pending.swap(true, Ordering::Relaxed) {
                return Err(HostAudioBackendError::BackendFailure {
                    message: "already prepared".into(),
                });
            }
            Ok(())
        }
        fn cancel_prepared_graph_attachments(&self) {
            self.pending.store(false, Ordering::Relaxed);
            self.cancellations.fetch_add(1, Ordering::Relaxed);
        }
    }
    struct FailResources;
    impl GraphResourceBackend for FailResources {
        fn reconcile(
            &self,
            _: &GraphDocument,
            _: crate::PlaybackBindings<'_>,
        ) -> Result<(), HostAudioBackendError> {
            Err(HostAudioBackendError::BackendFailure {
                message: "missing native device".into(),
            })
        }
    }
    let fixture = Fixture::new();
    let mut engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let endpoint = Arc::new(Endpoint {
        inner: ContractOnlyVirtualEndpointBackend::default(),
        pending: false.into(),
        cancellations: AtomicU64::new(0),
    });
    engine.endpoint_backend = endpoint.clone();
    engine.graph_resources = Arc::new(FailResources);
    let change = request(&engine, GraphCatalogAction::Create { name: "new".into() });
    engine.mutate_graph_catalog(change).unwrap();
    assert_eq!(endpoint.cancellations.load(Ordering::Relaxed), 0);
    assert!(!endpoint.pending.load(Ordering::Relaxed));
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    let selected = engine.graph();
    let change = default_duplex_document(selected.revision + 1, Some(-6.0));
    assert!(engine.apply_graph_request(change.clone().into()).is_err());
    assert_eq!(endpoint.cancellations.load(Ordering::Relaxed), 1);
    assert!(!endpoint.pending.load(Ordering::Relaxed));
    assert_eq!(engine.graph(), selected);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        before
    );
    engine.graph_resources = Arc::new(NoopGraphResourceBackend);
    engine.apply_graph_request(change.into()).unwrap();
    assert!(endpoint.pending.load(Ordering::Relaxed));
    // A different edit while the successful processor is still pending must not cancel it.
    let mut document = engine.graph();
    document.revision += 1;
    assert_eq!(
        engine
            .apply_graph_request(document.into())
            .unwrap_err()
            .code,
        ErrorCode::GraphExchangeBusy
    );
    assert_eq!(endpoint.cancellations.load(Ordering::Relaxed), 1);
    settle(&engine);
}

#[test]
fn microphone_consent_failures_preserve_the_graph_and_allow_explicit_retry() {
    struct Consent(HostAudioBackendError);
    impl GraphResourceBackend for Consent {
        fn reconcile(
            &self,
            _: &GraphDocument,
            _: crate::PlaybackBindings<'_>,
        ) -> Result<(), HostAudioBackendError> {
            Err(self.0.clone())
        }
    }
    for (failure, code) in [
        (
            HostAudioBackendError::PermissionPending {
                capability: "microphone".into(),
            },
            ErrorCode::AudioPermissionPending,
        ),
        (
            HostAudioBackendError::PermissionDenied {
                capability: "microphone".into(),
            },
            ErrorCode::AudioPermissionDenied,
        ),
    ] {
        let fixture = Fixture::new();
        let mut engine = controller(default_duplex_document(1, None))
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        let before = engine.graph();
        let disk = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
        engine.graph_resources = Arc::new(Consent(failure));
        let proposed = default_duplex_document(before.revision + 1, Some(-6.0));
        let error = engine
            .apply_graph_request(proposed.clone().into())
            .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.parameters["configurationOutcome"], "not_applied");
        assert_eq!(error.parameters["capability"], "microphone");
        assert_eq!(engine.graph(), before);
        assert_eq!(engine.exchange.revisions().submitted, before.revision);
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            disk
        );
        engine.graph_resources = Arc::new(NoopGraphResourceBackend);
        engine.apply_graph_request(proposed.clone().into()).unwrap();
        settle(&engine);
        assert_eq!(engine.graph(), proposed);
    }
}

#[test]
fn duplicate_identity_or_unsupported_schema_never_gets_reinitialized() {
    for version in [1, 9] {
        let fixture = Fixture::new();
        let engine = controller(default_duplex_document(1, None))
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        drop(engine);
        let mut saved: serde_json::Value = serde_json::from_slice(
            &crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        )
        .unwrap();
        saved["version"] = version.into();
        let duplicate = saved["graphs"][0].clone();
        saved["graphs"].as_array_mut().unwrap().push(duplicate);
        let bytes = serde_json::to_vec(&saved).unwrap();
        fixture.0.files.insert(fixture.path(), bytes.clone());
        let engine = controller(default_duplex_document(1, None))
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        assert!(engine.graph_catalog().is_err());
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            bytes
        );
    }
}

#[test]
fn parameter_storage_failures_keep_native_edit_and_later_save_recovers_all_ids() {
    for failure_at_commit in [false, true] {
        let fixture = Fixture::new();
        let (engine, prepares) = parameter_fixture(parameter_document());
        let engine = engine
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        let original =
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
        if failure_at_commit {
            fixture
                .0
                .files
                .reject_commit(fixture.path(), std::io::ErrorKind::PermissionDenied);
        } else {
            fixture
                .0
                .files
                .reject_stage(fixture.path(), std::io::ErrorKind::NotADirectory);
        }
        let publications = AtomicU64::new(0);
        let accepted = engine
            .apply_processor_parameter_update("effect", 7, 0.25, |_, _| {
                publications.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .unwrap();
        assert!(accepted.persistence_error.is_some());
        assert_eq!(publications.load(Ordering::Relaxed), 1);
        assert_eq!(
            provider_parameters(&engine.graph())["parameterValues"]["7"],
            0.25
        );
        assert_eq!(
            crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
            original
        );
        assert!(engine.graph_catalog().unwrap().persistence_error.is_some());
        fixture.0.files.allow_stage(&fixture.path());
        fixture.0.files.allow_commit(&fixture.path());
        let recovered = engine
            .apply_processor_parameter_update("effect", 9, 0.75, |_, _| Ok(()))
            .unwrap();
        assert!(recovered.persistence_error.is_none());
        assert!(engine.graph_catalog().unwrap().persistence_error.is_none());
        assert_eq!(prepares.load(Ordering::Relaxed), 0);
        drop(engine);
        let (restarted, _) = parameter_fixture(GraphDocument::new(1, vec![], vec![]));
        let restarted = restarted
            .with_test_documents(&fixture.0.files)
            .with_graph_catalog_path(fixture.path());
        assert_eq!(
            provider_parameters(&restarted.graph())["parameterValues"],
            serde_json::json!({"7": 0.25, "9": 0.75})
        );
    }
}

#[test]
fn failed_stage_leaves_engine_and_catalog_unmodified() {
    let fixture = Fixture::new();
    let engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    fixture
        .0
        .files
        .reject_stage(fixture.path(), std::io::ErrorKind::NotADirectory);
    let document = engine.graph();
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    assert!(
        engine
            .mutate_graph_catalog(request(
                &engine,
                GraphCatalogAction::Create { name: "new".into() }
            ))
            .is_err()
    );
    assert_eq!(engine.graph(), document);
    assert_eq!(engine.graph_catalog().unwrap().graphs.len(), 1);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        before
    );
}

#[test]
fn rename_failure_after_native_prepare_publishes_audio_and_warns_without_orphaning_resources() {
    struct CountPrepared(AtomicU64);
    impl GraphResourceBackend for CountPrepared {
        fn reconcile(
            &self,
            _: &GraphDocument,
            _: crate::PlaybackBindings<'_>,
        ) -> Result<(), HostAudioBackendError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    let fixture = Fixture::new();
    let mut engine = controller(default_duplex_document(1, None))
        .with_test_documents(&fixture.0.files)
        .with_graph_catalog_path(fixture.path());
    let resources = Arc::new(CountPrepared(AtomicU64::new(0)));
    engine.graph_resources = resources.clone();
    let created = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Create {
                name: "published but unsaved".into(),
            },
        ))
        .unwrap();
    assert_eq!(resources.0.load(Ordering::Relaxed), 0);
    let before = crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap();
    fixture
        .0
        .files
        .reject_commit(fixture.path(), std::io::ErrorKind::IsADirectory);
    let document = default_duplex_document(engine.graph().revision + 1, Some(-6.0));
    let applied = engine.apply_graph_request(document.clone().into()).unwrap();
    assert_eq!(resources.0.load(Ordering::Relaxed), 1);
    assert!(applied.persistence_error.is_some());
    assert_eq!(engine.graph(), document);
    assert_eq!(engine.graph_catalog().unwrap().active_id, created.active_id);
    assert_eq!(
        crate::test_support::read_document(&fixture.0.files, &fixture.path()).unwrap(),
        before
    );
    settle(&engine); // Processor publication was not rejected after resources were staged.
    let unchanged = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Select {
                id: created.active_id,
            },
        ))
        .unwrap();
    assert!(unchanged.persistence_error.is_some());
    fixture.0.files.allow_stage(&fixture.path());
    fixture.0.files.allow_commit(&fixture.path());
    let mut document = engine.graph();
    document.revision += 1;
    engine.apply_graph_request(document.into()).unwrap();
    settle(&engine);
    assert!(engine.graph_catalog().unwrap().persistence_error.is_none());
    assert_eq!(resources.0.load(Ordering::Relaxed), 2);
}
