use super::*;

fn graph() -> GraphDocument {
    serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,
        "nodes":[{"id":"gain","type":"gain","config":{"channels":2,"gainDb":-3.0}},
            {"id":"record","type":"recorder","config":{"channels":2}}],
        "edges":[{"id":"wire","source":{"nodeId":"gain","portId":"audio"},
            "destination":{"nodeId":"record","portId":"audio"}}]
    }))
    .unwrap()
}

fn request(revision: u64, enabled: Option<bool>, muted: Option<bool>) -> NodeControlRequest {
    NodeControlRequest {
        expected_revision: revision,
        enabled,
        muted,
        gain_db: None,
    }
}

fn acknowledge_control(
    engine: &EngineGraphController,
    processor: &mut Box<mixrelay_render::BlockProcessor>,
) {
    let plan = engine.exchange.plan_revisions();
    assert!(
        !engine.exchange.activate_pending(processor),
        "control must not replace topology"
    );
    processor
        .process_block(128, &[], &mut [])
        .expect("headless audio block");
    assert_eq!(engine.exchange.plan_revisions(), plan);
    let revision = engine.exchange.revisions();
    assert_eq!(
        revision.active, revision.submitted,
        "audio block acknowledges control state"
    );
}

#[test]
fn control_publication_work_baseline() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct CountedFactory {
        inner: Arc<dyn crate::ProcessorFactory>,
        prepares: Arc<AtomicUsize>,
        compilers: Arc<AtomicUsize>,
    }
    impl crate::ProcessorFactory for CountedFactory {
        fn compiler(&self) -> mixrelay_audio_graph::GraphCompiler {
            self.compilers.fetch_add(1, Ordering::Relaxed);
            self.inner.compiler()
        }
        fn prepare(
            &self,
            execution: &mixrelay_audio_graph::ExecutionPlan,
        ) -> Result<Box<mixrelay_render::BlockProcessor>, crate::PrepareProcessorError> {
            self.prepares.fetch_add(1, Ordering::Relaxed);
            self.inner.prepare(execution)
        }
    }
    let directory = crate::test_support::Documents::new("control-work-count");
    let mut document = graph();
    for index in 0..62 {
        let mut extra = document.nodes[0].clone();
        extra.id = format!("extra-{index}").as_str().into();
        document.nodes.push(extra);
    }
    let mut engine = crate::endpoint_settings_tests::controller(document)
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(directory.join("graphs.json"));
    let id = engine.graph_catalog().unwrap().active_id;
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    let prepares = Arc::new(AtomicUsize::new(0));
    let compilers = Arc::new(AtomicUsize::new(0));
    let inner = engine.factory.clone();
    engine.factory = Arc::new(CountedFactory {
        inner,
        prepares: prepares.clone(),
        compilers: compilers.clone(),
    });
    let topology = engine.exchange.plan_revisions();
    let start = std::time::Instant::now();
    for index in 0..100 {
        let patch = request(
            engine.exchange.revisions().submitted,
            None,
            Some(index % 2 == 0),
        );
        assert!(
            engine
                .preview_node_switch(&id, "gain", &patch)
                .unwrap()
                .capture_manifest
                .is_none()
        );
        engine.change_node_control(&id, "gain", &patch).unwrap();
        acknowledge_control(&engine, &mut processor);
        engine.exchange.reclaim_retired();
    }
    eprintln!(
        "control-work: nodes=64 updates=100 sample-rate=48000 block-frames=128 prepare={} elapsed-us={}",
        prepares.load(Ordering::Relaxed),
        start.elapsed().as_micros()
    );
    assert_eq!(prepares.load(Ordering::Relaxed), 0);
    assert_eq!(compilers.load(Ordering::Relaxed), 0);
    assert_eq!(engine.exchange.plan_revisions(), topology);
}

#[test]
fn first_unprepared_activation_builds_once_then_controls_reuse_the_published_plan() {
    let directory = crate::test_support::Documents::new("first-node-activation");
    let mut initial = graph();
    initial.nodes[0].control.enabled = false;
    let engine = crate::endpoint_settings_tests::controller(initial)
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(directory.join("graphs.json"));
    let graph_id = engine.graph_catalog().unwrap().active_id;
    let revision = engine.exchange.revisions().submitted;
    let on = request(revision, Some(true), None);
    let preview = engine.preview_node_switch(&graph_id, "gain", &on).unwrap();
    let manifest = preview
        .capture_manifest
        .expect("unprepared runtime requires publication");
    assert_eq!(manifest.revision, revision + 1);
    let response = engine.change_node_control(&graph_id, "gain", &on).unwrap();
    assert_eq!(response.application.revision, manifest.revision);
    assert!(response.requires_graph_reload);
    let mut processor = crate::controller_test_support::settle(&engine);
    let topology = engine.exchange.plan_revisions();
    for enabled in [false, true, false, true] {
        let patch = request(engine.exchange.revisions().submitted, Some(enabled), None);
        assert!(
            engine
                .preview_node_switch(&graph_id, "gain", &patch)
                .unwrap()
                .capture_manifest
                .is_none()
        );
        engine
            .change_node_control(&graph_id, "gain", &patch)
            .unwrap();
        acknowledge_control(&engine, &mut processor);
        assert_eq!(engine.exchange.plan_revisions(), topology);
    }
}

#[test]
fn native_restoration_keeps_reads_available_and_bounds_write_admission() {
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest};
    use mixrelay_control_protocol::RecorderControlRequest;
    let engine = std::sync::Arc::new(crate::endpoint_settings_tests::controller(graph()));
    let held = engine.mutation.lock();
    let (send, receive) = std::sync::mpsc::channel();
    let worker_engine = engine.clone();
    let worker = std::thread::spawn(move || {
        let snapshot = worker_engine.controls_snapshot().unwrap();
        let graph_id = &snapshot.graphs[0].id;
        let controls = worker_engine.read_node_controls(graph_id).unwrap();
        let recorder = worker_engine.recorder_status(graph_id, "record").unwrap();
        let patch = request(1, Some(false), None);
        let errors = [
            worker_engine
                .preview_node_switch(graph_id, "gain", &patch)
                .unwrap_err(),
            worker_engine
                .change_node_control(graph_id, "gain", &patch)
                .unwrap_err(),
            worker_engine
                .control_recorder(
                    graph_id,
                    "record",
                    RecorderControlRequest {
                        expected_revision: 1,
                        expected_recording: None,
                        request: RecorderRequest {
                            action: RecorderAction::Stop,
                            path: None,
                            sample_rate: None,
                            encoding: None,
                        },
                    },
                )
                .unwrap_err(),
        ];
        send.send((controls, recorder, errors)).unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(2));
    drop(held);
    worker.join().unwrap();
    let (controls, recorder, errors) =
        result.expect("control requests must finish while restoration owns mutation");
    assert_eq!(controls.nodes.len(), 2);
    assert_eq!(
        recorder.state,
        mixrelay_audio_io::recording::RecorderState::Stopped
    );
    for error in errors {
        assert_eq!(error.code, ErrorCode::GraphExchangeBusy);
    }
}

#[test]
fn gain_is_discovered_and_validated_before_publication() {
    let directory = crate::test_support::Documents::new("node-gain");
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(directory.join("graphs.json"));
    let id = engine.graph_catalog().unwrap().active_id;
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    let revision = engine.exchange.revisions().submitted;
    assert_eq!(
        engine.read_node_controls(&id).unwrap().nodes[0].gain_db,
        Some(-3.0)
    );
    let original = engine.graph();
    for (node, gain) in [
        ("gain", f32::NAN),
        ("gain", 13.0),
        ("gain", -61.0),
        ("record", 0.0),
    ] {
        let mut patch = request(revision, None, None);
        patch.gain_db = Some(gain);
        assert_eq!(
            engine
                .change_node_control(&id, node, &patch)
                .unwrap_err()
                .code,
            ErrorCode::GraphInvalid
        );
        assert_eq!(engine.graph(), original);
    }
    let mut patch = request(revision, None, None);
    patch.gain_db = Some(-9.0);
    engine.change_node_control(&id, "gain", &patch).unwrap();
    let pending = engine.controls_snapshot().unwrap();
    assert_eq!(pending.revision, revision + 1);
    assert_eq!(pending.active_revision, revision);
    acknowledge_control(&engine, &mut processor);
    assert_eq!(
        engine.controls_snapshot().unwrap().active_revision,
        revision + 1
    );
    assert_eq!(
        engine.read_node_controls(&id).unwrap().nodes[0].gain_db,
        Some(-9.0)
    );
    assert_eq!(engine.graph().edges, original.edges);
}

#[test]
fn preview_is_read_only_and_switches_preserve_authored_connections() {
    let directory = crate::test_support::Documents::new("node-controls");
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(directory.join("graphs.json"));
    let catalog = engine.graph_catalog().unwrap();
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    let original = engine.graph();
    let update = request(
        engine.exchange.revisions().submitted,
        Some(false),
        Some(true),
    );
    let preview = engine
        .preview_node_switch(&catalog.active_id, "gain", &update)
        .unwrap();
    assert_eq!(engine.graph(), original);
    let applied = engine
        .change_node_control(&catalog.active_id, "gain", &update)
        .unwrap();
    assert!(preview.capture_manifest.is_none());
    assert!(!applied.requires_graph_reload);
    acknowledge_control(&engine, &mut processor);
    assert_eq!(engine.graph().edges, original.edges);
    let composed = engine
        .compose_execution(
            &engine.graph(),
            engine.catalog_candidate(&engine.graph()).as_ref(),
            *engine.context.read(),
        )
        .unwrap();
    assert_eq!(composed.document.nodes.len(), original.nodes.len());
    assert_eq!(composed.document.edges.len(), original.edges.len());
    let controls = engine.read_node_controls(&catalog.active_id).unwrap();
    assert!(!controls.nodes[0].control.enabled);
    assert!(controls.nodes[0].control.muted);
    engine
        .change_node_control(
            &catalog.active_id,
            "gain",
            &request(applied.application.revision, Some(true), None),
        )
        .unwrap();
    acknowledge_control(&engine, &mut processor);
    assert_eq!(engine.graph().edges, original.edges);
    assert!(engine.graph().nodes[0].control.muted);
    let disk: serde_json::Value = serde_json::from_slice(
        &directory
            .files
            .bytes(&directory.join("graphs.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        disk["graphs"][0]["document"]["nodes"][0]["control"]["muted"],
        true
    );
}

#[test]
fn missing_stale_and_empty_commands_leave_catalog_untouched() {
    let directory = crate::test_support::Documents::new("node-control-errors");
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(directory.join("graphs.json"));
    let id = engine.graph_catalog().unwrap().active_id;
    let revision = engine.exchange.revisions().submitted;
    let original = engine.graph();
    assert_eq!(
        engine
            .change_node_control(&id, "deleted", &request(revision, Some(true), None))
            .unwrap_err()
            .code,
        ErrorCode::NodeNotFound
    );
    assert_eq!(
        engine
            .change_node_control(&id, "gain", &request(revision + 1, Some(true), None))
            .unwrap_err()
            .code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(
        engine
            .change_node_control(&id, "gain", &request(revision, None, None))
            .unwrap_err()
            .code,
        ErrorCode::GraphInvalid
    );
    assert_eq!(engine.graph(), original);
}

#[test]
fn background_node_switch_does_not_follow_editor_selection() {
    let directory = crate::test_support::Documents::new("node-control-graphs");
    let path = directory.join("graphs.json");
    directory.files.insert(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "version":1,"revision":1,"activeId":"a", "graphs":[
                {"id":"a","name":"A","document":graph(),"layoutRevision":0,"layout":{"positions":{}}},
                {"id":"b","name":"B","document":graph(),"layoutRevision":0,"layout":{"positions":{}}}
            ]
        }))
        .unwrap(),
    );
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(path);
    let revision = engine.exchange.revisions().submitted;
    let mut processor = crate::controller_test_support::settle(&engine);
    engine
        .change_node_control("b", "gain", &request(revision, Some(false), None))
        .unwrap();
    acknowledge_control(&engine, &mut processor);
    assert!(
        engine.read_node_controls("a").unwrap().nodes[0]
            .control
            .enabled
    );
    assert!(
        !engine.read_node_controls("b").unwrap().nodes[0]
            .control
            .enabled
    );
    assert_eq!(engine.graph_catalog().unwrap().active_id, "a");
}

fn recorder_start(revision: u64) -> mixrelay_control_protocol::RecorderControlRequest {
    mixrelay_control_protocol::RecorderControlRequest {
        expected_recording: None,
        expected_revision: revision,
        request: mixrelay_audio_io::recording::RecorderRequest {
            action: mixrelay_audio_io::recording::RecorderAction::Record,
            path: None,
            sample_rate: None,
            encoding: None,
        },
    }
}

#[test]
fn deleted_and_readded_identity_rejects_delayed_node_and_recorder_commands() {
    let directory = crate::test_support::Documents::new("node-control-recreated");
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(directory.join("graphs.json"));
    let id = engine.graph_catalog().unwrap().active_id;
    let old_revision = engine.exchange.revisions().submitted;
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    let mut deleted = engine.graph();
    deleted.revision += 1;
    deleted.nodes.retain(|node| node.id.as_str() != "record");
    deleted.edges.clear();
    engine.apply_graph_request(deleted.clone().into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(
        engine
            .change_node_control(&id, "record", &request(deleted.revision, Some(false), None))
            .unwrap_err()
            .code,
        ErrorCode::NodeNotFound
    );
    assert_eq!(
        engine
            .control_recorder(&id, "record", recorder_start(deleted.revision))
            .unwrap_err()
            .code,
        ErrorCode::NodeNotFound
    );
    assert_eq!(engine.graph(), deleted);
    let mut replaced = graph();
    replaced.revision = deleted.revision + 1;
    replaced.nodes[1].label = Some("New recorder instance".into());
    engine.apply_graph_request(replaced.clone().into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(
        engine
            .change_node_control(
                &id,
                "record",
                &request(old_revision, Some(false), Some(true))
            )
            .unwrap_err()
            .code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(
        engine
            .control_recorder(&id, "record", recorder_start(old_revision))
            .unwrap_err()
            .code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(engine.graph(), replaced);
    assert_eq!(
        engine.recorder_status(&id, "record").unwrap().state,
        mixrelay_audio_io::recording::RecorderState::Stopped
    );
    let off = engine
        .change_node_control(
            &id,
            "record",
            &request(replaced.revision, Some(false), None),
        )
        .unwrap();
    acknowledge_control(&engine, &mut processor);
    let disabled = engine.graph();
    assert_eq!(
        engine
            .control_recorder(&id, "record", recorder_start(off.application.revision))
            .unwrap_err()
            .code,
        ErrorCode::NodeDisabled
    );
    assert_eq!(engine.graph(), disabled);
    assert_eq!(
        engine.exchange.revisions().submitted,
        off.application.revision
    );
}
