//! Catalog graphs sharing device PCM across view switches, deletion and removal.
use super::*;

#[test]
fn controller_runs_both_graphs_and_view_switches_do_not_submit_or_interrupt_shared_pcm() {
    let directory = crate::test_support::Documents::new("composed-controller");
    let engine = directory.catalog(controller_with_driver_devices());
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    engine.apply_graph_request(graph_a(2).into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    let revision = engine.exchange.revisions();
    select(&engine, GraphCatalogAction::Create { name: "B".into() });
    let graph_b_id = engine.view_graph_id();
    assert_eq!(engine.exchange.revisions(), revision);
    engine.apply_graph_request(graph_b(3).into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_shared_pcm(&mut processor, &[DEVICE_A, DEVICE_B]);
    let revision = engine.exchange.revisions();
    for id in ["graph-1", &graph_b_id, "graph-1", &graph_b_id] {
        select(&engine, GraphCatalogAction::Select { id: id.into() });
        assert_eq!(engine.exchange.revisions(), revision);
        assert!(!engine.exchange.activate_pending(&mut processor));
        assert_shared_pcm(&mut processor, &[DEVICE_A, DEVICE_B]);
        assert!(
            engine
                .meters()
                .nodes
                .iter()
                .all(|meter| ["local", "output"].contains(&meter.node_id.as_str()))
        );
    }
    assert_eq!(engine.endpoint_settings().unwrap().devices.len(), 2);
}

#[test]
fn deleted_selection_is_committed_before_the_new_plan_can_activate() {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    let directory = crate::test_support::Documents::new("meter-selection-publication");
    let engine = directory.catalog(controller_with_driver_devices());
    engine.apply_graph_request(graph_a(2).into()).unwrap();
    let _first = crate::controller_test_support::settle(&engine);
    select(&engine, GraphCatalogAction::Create { name: "B".into() });
    engine.apply_graph_request(graph_b(3).into()).unwrap();
    let mut processor = crate::controller_test_support::settle(&engine);
    select(
        &engine,
        GraphCatalogAction::Select {
            id: "graph-1".into(),
        },
    );
    let document = graph_b(4);
    let candidate = serde_json::from_value(serde_json::json!({
        "version":1,"revision":4,"activeId":"remaining",
        "graphs":[{"id":"remaining","name":"B","document":document,"layoutRevision":0,"layout":{"positions":{}}}]
    }))
    .unwrap();
    let context = *engine.context.read();
    let PreparedExecution {
        processor: next_processor,
        view,
        ..
    } = engine
        .prepare_execution(&document, Some(&candidate), context)
        .unwrap();
    let bindings = next_processor.plan().meter_bindings().to_vec();
    let (started, observed) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    std::thread::scope(|scope| {
        let engine = &engine;
        let publisher = scope.spawn(move || {
            engine.exchange.submit_configured(next_processor, || {
                started.send(()).unwrap();
                blocked.recv().unwrap();
                engine.publish_execution_metadata(document.revision, bindings, view);
                *engine.graph.write() = document;
                engine.replace_catalog_memory(Some(candidate));
                Ok::<(), ()>(())
            })
        });
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        let (metadata_ready, pending_before_selection_commit) = engine.catalog.inspect(|_| {
            release.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let next = 4;
            while !engine.execution_views.read().contains_key(&next) && Instant::now() < deadline {
                std::thread::yield_now();
            }
            let metadata_ready = engine.execution_views.read().contains_key(&next);
            let pending_before_selection_commit = engine.exchange.has_pending();
            (metadata_ready, pending_before_selection_commit)
        });
        publisher.join().unwrap().unwrap();
        assert!(metadata_ready);
        assert!(
            !pending_before_selection_commit,
            "new topology cannot precede its selected graph"
        );
    });
    engine.refresh_control_snapshot();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_ne!(engine.view_graph_id(), "graph-1");
    assert_shared_pcm(&mut processor, &[DEVICE_B]);
    let snapshot = engine.meters();
    assert!(!snapshot.nodes.is_empty());
    assert_eq!(snapshot.revision, processor.plan().revision());
}

#[test]
fn self_device_routing_is_accepted_without_feedback_policy() {
    let directory = crate::test_support::Documents::new("composed-feedback");
    let engine = directory.catalog(controller_with_driver_devices());
    let mut processor = engine
        .prepare_and_validate(&engine.graph(), *engine.context.read())
        .unwrap()
        .processor;
    engine.apply_graph_request(graph_a(2).into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    select(&engine, GraphCatalogAction::Create { name: "B".into() });
    let mut feedback = graph_b(3);
    let NodeConfig::VirtualEndpoint(config) = &mut feedback.nodes[1].config else {
        panic!("device");
    };
    config.spec.id = VirtualEndpointId::from(DEVICE_A);
    config.spec.display_name = DEVICE_A.to_owned();
    engine.apply_graph_request(feedback.into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(engine.endpoint_settings().unwrap().devices.len(), 2);
}

#[test]
fn catalog_removal_keeps_shared_device_wiring_and_pcm_in_surviving_graph() {
    let directory = crate::test_support::Documents::new("remove-shared-controller");
    let engine = directory.catalog(controller_with_driver_devices());
    engine.apply_graph_request(graph_a(2).into()).unwrap();
    let mut processor = crate::controller_test_support::settle(&engine);
    select(
        &engine,
        GraphCatalogAction::Create {
            name: "Keep".into(),
        },
    );
    engine.apply_graph_request(graph_b(3).into()).unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_shared_pcm(&mut processor, &[DEVICE_A, DEVICE_B]);
    let before = engine.graph();
    select(
        &engine,
        GraphCatalogAction::Delete {
            id: "graph-1".into(),
        },
    );
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(engine.graph().nodes, before.nodes);
    assert_eq!(engine.graph().edges, before.edges);
    assert_eq!(engine.endpoint_settings().unwrap().devices.len(), 2);
    // Both duplex devices are still referenced by B. Their device-owned audio return path
    // survives deletion of A, as does B's explicit route between the devices.
    assert_shared_pcm(&mut processor, &[DEVICE_B]);
}
