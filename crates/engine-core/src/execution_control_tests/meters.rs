//! Meter reads, cached dynamics and their invalidation while execution changes.
use super::*;

#[test]
fn meters_respond_with_pcm_while_native_mutation_is_blocked() {
    use std::{sync::mpsc, time::Duration};

    let engine = controller_with_driver_devices();
    engine.apply_graph_request(graph_b(2).into()).unwrap();
    let mut processor = crate::controller_test_support::settle(&engine);
    assert_shared_pcm(&mut processor, &[DEVICE_B]);
    let snapshot = std::thread::scope(|scope| {
        let mutation = engine.mutation.lock();
        let (sender, receiver) = mpsc::channel();
        let started = std::time::Instant::now();
        let engine = &engine;
        let reader = scope.spawn(move || sender.send(engine.meters()).unwrap());
        let result = receiver.recv_timeout(Duration::from_secs(2));
        eprintln!(
            "blocked-mutation meter read: {:?}, responded={}",
            started.elapsed(),
            result.is_ok()
        );
        drop(mutation);
        reader.join().unwrap();
        result.expect("meters must not wait for a native graph mutation")
    });
    assert_eq!(snapshot.revision, processor.plan().revision());
    assert!(!snapshot.nodes.is_empty());
    assert!(
        snapshot
            .nodes
            .iter()
            .any(|node| node.peaks.iter().any(|peak| *peak > 0.0))
    );
    assert_shared_pcm(&mut processor, &[DEVICE_B]);
}

#[test]
fn meters_report_applied_controls_without_changing_the_topology_revision() {
    let directory = crate::test_support::Documents::new("meter-control-revision");
    let engine = directory.catalog(controller_with_driver_devices());
    engine.apply_graph_request(graph_b(2).into()).unwrap();
    let mut processor = crate::controller_test_support::settle(&engine);
    let topology = processor.plan().revision();
    let response = engine
        .change_node_control(
            "graph-1",
            "local",
            &mixrelay_control_protocol::NodeControlRequest {
                expected_revision: topology,
                enabled: None,
                muted: None,
                gain_db: Some(-6.0),
            },
        )
        .unwrap();
    assert!(!response.requires_graph_reload);
    let frames = processor.plan().format().block_frames;
    let inputs = vec![vec![0.25; frames as usize * 2]; processor.plan().source_bindings().len()];
    let mut outputs = vec![vec![0.0; frames as usize * 2]; processor.plan().sink_bindings().len()];
    for _ in 0..2 {
        processor
            .process_block(
                frames,
                &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                &mut outputs
                    .iter_mut()
                    .map(Vec::as_mut_slice)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    let snapshot = engine.meters();
    assert_eq!(processor.plan().revision(), topology);
    assert_eq!(snapshot.revision, engine.exchange.revisions().active);
    assert!(snapshot.revision > topology);
    assert!(
        snapshot
            .nodes
            .iter()
            .any(|node| node.peaks.iter().any(|peak| *peak > 0.0))
    );
}

fn dynamics_meter_fixture() -> (
    EngineGraphController,
    Box<BlockProcessor>,
    crate::test_support::Documents,
) {
    let directory = crate::test_support::Documents::new("meter-dynamics-reading");
    let engine = directory.catalog(controller_with_driver_devices());
    let mut graph = graph_b(2);
    graph.nodes.push(NodeSpec {
        id: "limiter-user".into(),
        label: Some("Limiter".into()),
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::Limiter(mixrelay_audio_graph::LimiterConfig {
            version: 1,
            channels: 2,
            bypass: false,
            ceiling_db: -6.0,
            release_ms: 100.0,
        }),
    });
    let mut input = route(("local", "from_os"), ("limiter-user", "audio"));
    input.id = "dynamics-input".into();
    let mut output = route(("limiter-user", "audio"), ("output", "to_os"));
    output.id = "dynamics-output".into();
    graph.edges = vec![input, output];
    engine.apply_graph_request(graph.into()).unwrap();
    let processor = crate::controller_test_support::settle(&engine);
    (engine, processor, directory)
}

#[test]
fn meters_publish_actual_dynamics_for_the_visible_native_node_identity() {
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    let frames = processor.plan().format().block_frames;
    let inputs = vec![vec![1.0; frames as usize * 2]; processor.plan().source_bindings().len()];
    let mut outputs = vec![vec![0.0; frames as usize * 2]; processor.plan().sink_bindings().len()];
    for _ in 0..3 {
        processor
            .process_block(
                frames,
                &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                &mut outputs
                    .iter_mut()
                    .map(Vec::as_mut_slice)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    let snapshot = engine.meters();
    let meter = snapshot
        .nodes
        .iter()
        .find(|meter| meter.node_id == "limiter-user")
        .expect("local dynamics meter identity");
    let dynamics = meter.dynamics.as_ref().expect("actual dynamics reading");
    assert_eq!(meter.port_id, "audio");
    assert!((dynamics.threshold_db + 6.0).abs() < 0.001);
    assert!((dynamics.gain_reduction_db - 6.0).abs() < 0.001);
    assert!(
        dynamics
            .input_peaks
            .iter()
            .all(|peak| (*peak - 1.0).abs() < 0.001)
    );
    assert!(
        meter
            .peaks
            .iter()
            .all(|peak| (*peak - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001)
    );
}

fn meter_block(processor: &mut BlockProcessor, sample: f32) {
    meter_frames(processor, sample, processor.plan().format().block_frames);
}

fn meter_frames(processor: &mut BlockProcessor, sample: f32, frames: u32) {
    let inputs = vec![vec![sample; frames as usize * 2]; processor.plan().source_bindings().len()];
    let mut outputs = vec![vec![0.0; frames as usize * 2]; processor.plan().sink_bindings().len()];
    processor
        .process_block(
            frames,
            &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            &mut outputs
                .iter_mut()
                .map(Vec::as_mut_slice)
                .collect::<Vec<_>>(),
        )
        .unwrap();
}

#[test]
fn concurrent_meter_readers_share_a_window_without_consuming_each_others_dynamics() {
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let now = mixrelay_time::Instant::from_origin(std::time::Duration::ZERO);
    let snapshots = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| engine.meter_snapshot_at(now)))
            .collect();
        readers
            .into_iter()
            .map(|reader| reader.join().unwrap())
            .collect::<Vec<_>>()
    });
    let dynamics = snapshots[0]
        .nodes
        .iter()
        .find(|node| node.node_id == "limiter-user")
        .unwrap()
        .dynamics
        .as_ref()
        .unwrap();
    assert!((dynamics.gain_reduction_db - 6.0).abs() < 0.001);
    assert!(snapshots.iter().all(|snapshot| snapshot == &snapshots[0]));
    // A reader timestamped before a lock wait can arrive after a newer request.
    meter_block(&mut processor, 1.0);
    let later = engine.meter_snapshot_at(now.saturating_add(std::time::Duration::from_millis(100)));
    let delayed =
        engine.meter_snapshot_at(now.saturating_add(std::time::Duration::from_millis(20)));
    assert_eq!(delayed, later);
    // No callback after the window: waiting, rather than a fabricated GR=0 reading.
    let waiting =
        engine.meter_snapshot_at(now.saturating_add(std::time::Duration::from_millis(200)));
    assert!(
        waiting
            .nodes
            .iter()
            .all(|node| node.dynamics.is_none() && node.peaks.iter().all(|peak| *peak == 0.0))
    );
    // A completed silent callback is real telemetry, unlike the waiting case above.
    meter_block(&mut processor, 0.0);
    meter_block(&mut processor, 0.0);
    // Drain the prepared device-bus quantum at the silence transition.
    let _transition =
        engine.meter_snapshot_at(now.saturating_add(std::time::Duration::from_millis(200)));
    meter_block(&mut processor, 0.0);
    meter_block(&mut processor, 0.0);
    let silent =
        engine.meter_snapshot_at(now.saturating_add(std::time::Duration::from_millis(300)));
    let silent = silent
        .nodes
        .iter()
        .find(|node| node.node_id == "limiter-user")
        .unwrap();
    assert!(silent.peaks.iter().all(|peak| *peak == 0.0));
    assert_eq!(silent.dynamics.as_ref().unwrap().input_peaks, [0.0, 0.0]);
}

#[test]
fn off_and_applied_control_changes_invalidate_the_cached_reading_immediately() {
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let now = mixrelay_time::Instant::from_origin(std::time::Duration::ZERO);
    assert!(
        engine
            .meter_snapshot_at(now)
            .nodes
            .iter()
            .any(|node| node.dynamics.is_some())
    );
    engine.power.set_enabled(false);
    assert!(
        engine
            .meter_snapshot_at(now)
            .nodes
            .iter()
            .all(|node| node.dynamics.is_none())
    );
    engine.power.set_enabled(true);
    assert!(
        engine
            .meter_snapshot_at(now)
            .nodes
            .iter()
            .all(|node| node.dynamics.is_none())
    );
    meter_block(&mut processor, 1.0);
    assert!(
        engine
            .meter_snapshot_at(now)
            .nodes
            .iter()
            .any(|node| node.dynamics.is_some())
    );
    let revision = engine.graph().revision;
    engine
        .change_node_control(
            "graph-1",
            "limiter-user",
            &mixrelay_control_protocol::NodeControlRequest {
                expected_revision: revision,
                enabled: Some(false),
                muted: None,
                gain_db: None,
            },
        )
        .unwrap();
    // Applying controls without any PCM frames must not stamp the previous block.
    meter_frames(&mut processor, 0.0, 0);
    let waiting = engine.meter_snapshot_at(now);
    assert!(waiting.revision > revision);
    assert!(waiting.nodes.iter().all(|node| node.dynamics.is_none()));
    meter_block(&mut processor, 1.0);
    let snapshot = engine.meter_snapshot_at(now);
    assert!(snapshot.revision > revision);
    let reading = snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == "limiter-user")
        .unwrap()
        .dynamics
        .as_ref()
        .unwrap();
    assert!(reading.gain_reduction_db.abs() < 0.001);
    assert_eq!(reading.input_peaks, [0.0, 0.0]);
}

#[test]
fn a_callback_during_capture_is_rejected_until_a_later_complete_block() {
    use crate::meter_snapshot::{MeterKey, MeterPublisher};
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let key = MeterKey {
        graph_id: "graph-1".into(),
        plan_revision: 2,
        submitted_revision: 2,
        applied_revision: 2,
        enabled: true,
    };
    let binding = processor
        .plan()
        .meter_bindings()
        .iter()
        .position(mixrelay_render::MeterBinding::has_dynamics)
        .unwrap();
    let now = mixrelay_time::Instant::from_origin(std::time::Duration::ZERO);
    let mut publisher = MeterPublisher::new(std::sync::Arc::clone(&engine.meters), now);
    let capture = |measured: bool| MeterSnapshot {
        protocol_version: PROTOCOL_VERSION,
        revision: 2,
        nodes: vec![NodeMeterSnapshot {
            node_id: "limiter-user".into(),
            port_id: "audio".into(),
            peaks: if measured {
                engine.meters.take_peaks(2, binding, 2)
            } else {
                vec![0.0; 2]
            },
            dynamics: if measured {
                engine.meters.take_dynamics(2, binding, 2).map(|value| {
                    mixrelay_control_protocol::DynamicsMeterSnapshot {
                        input_peaks: value.input_peaks,
                        gain_reduction_db: value.gain_reduction_db,
                        threshold_db: value.threshold_db,
                    }
                })
            } else {
                None
            },
        }],
    };
    let raced = publisher.read(
        now,
        key.clone(),
        |measured| {
            if measured {
                meter_block(&mut processor, 1.0);
            }
            capture(measured)
        },
        || true,
    );
    assert!(raced.nodes[0].dynamics.is_none());
    let waiting = publisher.read(now, key.clone(), capture, || true);
    assert!(waiting.nodes[0].dynamics.is_none());
    meter_block(&mut processor, 1.0);
    let fresh = publisher.read(now, key, capture, || true);
    let reading = fresh.nodes[0].dynamics.as_ref().unwrap();
    assert_eq!(reading.input_peaks, [1.0, 1.0]);
    assert!((reading.gain_reduction_db - 6.0).abs() < 0.001);
}

#[test]
fn threshold_revision_and_node_removal_cannot_reuse_the_previous_cached_gain() {
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let now = mixrelay_time::Instant::from_origin(std::time::Duration::ZERO);
    let old = engine.meter_snapshot_at(now);
    assert!(
        (old.nodes
            .iter()
            .find(|node| node.node_id == "limiter-user")
            .unwrap()
            .dynamics
            .as_ref()
            .unwrap()
            .threshold_db
            + 6.0)
            .abs()
            < 0.001
    );
    let mut graph = engine.graph();
    graph.revision += 1;
    let NodeConfig::Limiter(config) = &mut graph
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "limiter-user")
        .unwrap()
        .config
    else {
        panic!("limiter config")
    };
    config.ceiling_db = -12.0;
    engine.apply_graph_request(graph.into()).unwrap();
    assert!(
        engine
            .meter_snapshot_at(now)
            .nodes
            .iter()
            .all(|node| node.dynamics.is_none())
    );
    assert!(engine.exchange.activate_pending(&mut processor));
    assert!(
        engine
            .meter_snapshot_at(now)
            .nodes
            .iter()
            .all(|node| node.dynamics.is_none())
    );
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let new = engine.meter_snapshot_at(now);
    let reading = new
        .nodes
        .iter()
        .find(|node| node.node_id == "limiter-user")
        .unwrap()
        .dynamics
        .as_ref()
        .unwrap();
    assert!((reading.threshold_db + 12.0).abs() < 0.001);
    assert!((reading.gain_reduction_db - 12.0).abs() < 0.001);
    let next_revision = engine.graph().revision + 1;
    engine
        .apply_graph_request(graph_b(next_revision).into())
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    meter_block(&mut processor, 1.0);
    let removed = engine.meter_snapshot_at(now);
    assert!(
        removed
            .nodes
            .iter()
            .all(|node| node.node_id != "limiter-user" && node.dynamics.is_none())
    );
}

#[test]
fn meter_publisher_and_cached_nodes_release_the_bank_with_the_controller() {
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    let weak = std::sync::Arc::downgrade(&engine.meters);
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let snapshot = engine.meters();
    assert!(snapshot.nodes.iter().any(|node| node.dynamics.is_some()));
    drop(processor);
    drop(engine);
    assert!(weak.upgrade().is_none());
    // Keeping a serialized response has no ownership over native banks or plans.
    assert!(!snapshot.nodes.is_empty());
}

#[test]
fn simultaneous_completed_blocks_and_readers_never_publish_partially_drained_unity() {
    let (engine, mut processor, _directory) = dynamics_meter_fixture();
    for _ in 0..3 {
        meter_block(&mut processor, 1.0);
    }
    let frames = processor.plan().format().block_frames;
    let inputs = vec![vec![1.0; frames as usize * 2]; processor.plan().source_bindings().len()];
    let mut outputs = vec![vec![0.0; frames as usize * 2]; processor.plan().sink_bindings().len()];
    let now = mixrelay_time::Instant::from_origin(std::time::Duration::ZERO);
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            for _ in 0..256 {
                processor
                    .process_block(
                        frames,
                        &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                        &mut outputs
                            .iter_mut()
                            .map(Vec::as_mut_slice)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
            }
        });
        let readers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    for window in 0..256 {
                        let snapshot = engine.meter_snapshot_at(
                            now.saturating_add(std::time::Duration::from_millis(window * 50)),
                        );
                        let node = snapshot
                            .nodes
                            .iter()
                            .find(|node| node.node_id == "limiter-user")
                            .unwrap();
                        if let Some(reading) = &node.dynamics {
                            assert!((reading.gain_reduction_db - 6.0).abs() < 0.001);
                            assert_eq!(reading.input_peaks, [1.0, 1.0]);
                        }
                    }
                })
            })
            .collect();
        for reader in readers {
            reader.join().unwrap();
        }
    });
    // A complete block after every contested drain restores a truthful reading.
    meter_block(&mut processor, 1.0);
    let snapshot = engine.meter_snapshot_at(now.saturating_add(std::time::Duration::from_secs(20)));
    let reading = snapshot
        .nodes
        .iter()
        .find(|node| node.node_id == "limiter-user")
        .unwrap()
        .dynamics
        .as_ref()
        .unwrap();
    assert!((reading.gain_reduction_db - 6.0).abs() < 0.001);
}
