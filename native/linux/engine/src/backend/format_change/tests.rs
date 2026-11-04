use crate::{
    format::EndpointObservation,
    processing::run_audio_worker,
    runtime::{adapter_profile, create_initial_processor, render_format},
    test_support::{FixtureProcess, test_backend},
};
use mixrelay_audio_graph::{GraphDocument, NodeConfig};
use mixrelay_engine_core::{
    EngineGraphController, EngineGraphControllerConfig, default_duplex_document,
    session::AdapterRuntimeStatus,
};
use mixrelay_render::ProcessorExchange;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn native_pipewire_live_session_format_transaction() {
    let initial_document = GraphDocument::new(1, Vec::new(), Vec::new());
    let backend = test_backend();
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            adapter_profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_graph_format_control(),
    );
    let config = mixrelay_engine_core::test_util::configuration(render_format());
    let meters = config.meters.clone();
    let processor =
        create_initial_processor(&initial_document, meters.clone(), render_format()).unwrap();
    let exchange = Arc::new(ProcessorExchange::new(1));
    let controller = EngineGraphController::new_with_initial_document(
        exchange.clone(),
        EngineGraphControllerConfig {
            clock: std::sync::Arc::new(mixrelay_time::StdClock::new()),
            format: render_format(),
            meters,
            initial_bindings: processor.plan().meter_bindings().to_vec(),
            endpoint_backend: backend.clone(),
            required_endpoint_node_id: None,

            graph_resources: backend.clone(),
            runtime_status: status.clone(),

            ..config
        },
        initial_document,
    );
    let worker_status = status.clone();
    let worker_exchange = exchange.clone();
    let worker_backend = backend.clone();
    let worker = thread::spawn(move || {
        run_audio_worker(processor, worker_exchange, &worker_backend, &worker_status)
            .expect("native PipeWire worker failed");
    });
    let guard = mixrelay_engine_host::AudioWorker::new(status.clone(), worker);
    thread::sleep(Duration::from_millis(100));
    let mut document = default_duplex_document(1, None);
    let fixture = attach_fixture(&mut document);
    controller
        .apply_graph_request(document.clone().into())
        .unwrap();
    wait_for_audio(&controller, &exchange, 2);
    exchange.reclaim_retired();
    backend.reclaim_retired();
    for (sample_rate, block_frames) in [(44_100, 256), (48_000, 1024), (96_000, 128), (44_100, 64)]
    {
        let revision = activate_format(
            &controller,
            &exchange,
            backend.observation(),
            mixrelay_audio_io::session::SessionFormat {
                sample_rate,
                block_frames,
            },
        );
        let observed = controller.status();
        assert_eq!(observed.sample_rate, Some(sample_rate));
        assert_eq!(observed.buffer_frames, Some(block_frames));
        wait_for_audio(&controller, &exchange, revision);
        assert!(!status.stop_requested());
        assert_eq!(controller.graph().edges, document.edges);
        exchange.reclaim_retired();
        backend.reclaim_retired();
        let quantums = observed_quantums(backend.observation());
        println!(
            "activated_revision={revision}; native_endpoint_rate={sample_rate}; dsp_rate={sample_rate}; prepared_block={block_frames}; callback_quantums_graph_pcm={quantums:?}; native_worker_alive=true"
        );
    }
    assert_invalid_format_keeps_graph(&controller, &exchange);
    drop(guard);
    drop(fixture);
}

fn assert_invalid_format_keeps_graph(
    controller: &EngineGraphController,
    exchange: &ProcessorExchange,
) {
    let revision = exchange.revisions().active;
    let before = controller.graph();
    assert!(
        controller
            .reconfigure_session(mixrelay_control_protocol::SessionReconfigureRequest {
                expected_revision: revision,
                format: mixrelay_audio_io::session::SessionFormat {
                    sample_rate: 12_345,
                    block_frames: 128
                },
            })
            .is_err()
    );
    assert_eq!(controller.graph(), before);
    assert_eq!(exchange.revisions().active, revision);
}

fn activate_format(
    controller: &EngineGraphController,
    exchange: &ProcessorExchange,
    observation: &EndpointObservation,
    format: mixrelay_audio_io::session::SessionFormat,
) -> u64 {
    let response = controller
        .reconfigure_session(mixrelay_control_protocol::SessionReconfigureRequest {
            expected_revision: exchange.revisions().active,
            format,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while exchange.revisions().active != response.revision
        || observation.rate() != Some(format.sample_rate)
    {
        assert!(
            Instant::now() < deadline,
            "format activation timed out: {:?}",
            controller.status()
        );
        thread::sleep(Duration::from_millis(5));
    }
    response.revision
}

fn observed_quantums(observation: &EndpointObservation) -> std::collections::BTreeSet<(u64, u64)> {
    let mut result = std::collections::BTreeSet::new();
    for _ in 0..100 {
        let value = observation.quantum();
        if value != 0 {
            result.insert((value >> 32, value & u64::from(u32::MAX)));
        }
        thread::sleep(Duration::from_millis(1));
    }
    result
}

fn attach_fixture(document: &mut mixrelay_audio_graph::GraphDocument) -> FixtureProcess {
    let fixture = FixtureProcess::start(
        &std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap()),
        &[],
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !mixrelay_linux_audio_io::targets()
        .unwrap()
        .iter()
        .any(|target| target.application_id.as_deref() == Some("org.mixrelay.fixture.application"))
    {
        assert!(
            Instant::now() < deadline,
            "owned application fixture did not register"
        );
        thread::sleep(Duration::from_millis(5));
    }
    document.revision = 2;
    document.nodes.push(mixrelay_audio_graph::NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: "rate-fixture".into(), label: None,
        config: NodeConfig::ApplicationCapture(serde_json::from_value(serde_json::json!({
            "selector": mixrelay_audio_io::ApplicationSelector::BundleId { bundle_id: "org.mixrelay.fixture.application".into(), include_descendants: false },
            "stream": { "channels": 2, "channelLayout": "stereo" }, "sourcePlayback": "preserve"
        })).unwrap()),
    });
    for edge in &mut document.edges {
        edge.source.node_id = "rate-fixture".into();
        edge.source.port_id = mixrelay_audio_graph::AUDIO_PORT_ID.into();
    }
    fixture
}

fn wait_for_audio(controller: &EngineGraphController, exchange: &ProcessorExchange, revision: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let meters = controller.meters();
        if exchange.revisions().active == revision
            && meters.revision == revision
            && [
                "rate-fixture",
                mixrelay_engine_core::DEFAULT_ENDPOINT_NODE_ID,
            ]
            .iter()
            .all(|id| {
                meters
                    .nodes
                    .iter()
                    .any(|node| node.node_id == *id && node.peaks.iter().any(|peak| *peak > 0.05))
            })
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "live-format PCM not observed: {meters:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
