//! Actual service controller transactions over owned simulated `CoreAudio` resources.
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, DeviceSelector, GraphDocument, NodeConfig,
    NodeSpec, RenderFormat,
};
use mixrelay_control_server::ControlBackend;
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use mixrelay_engine_service::{EngineGraphController, EngineGraphControllerConfig};
use mixrelay_macos_engine::{
    MacDevices,
    test_support::{self, ActiveDevices as DeviceSet, CAPTURED, DEFAULT_BUFFER, SimulatedHost},
};
use mixrelay_render::{BlockProcessor, ProcessorExchange};
use std::{
    collections::BTreeSet,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
const EDITED_GAIN_DB: f32 = -6.0;
/// Waits for device workers. Only a stalled worker reaches the guard; the outcome itself
/// is decided by the observed state, never by elapsed time.
pub(super) fn wait_until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "native device transition stalled"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn simulated(rate: u32) -> (Arc<SimulatedHost>, String, MacDevices) {
    let host = SimulatedHost::new();
    let uid = host.plug("io.mixrelay.test.duplex", 2, 48_000, CAPTURED);
    let status = Arc::new(AdapterRuntimeStatus::new(
        test_support::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let resources = test_support::devices(status, rate, 256, host.clone());
    (host, uid, resources)
}

fn format(sample_rate: u32, block_frames: u32) -> RenderFormat {
    RenderFormat {
        sample_rate,
        block_frames,
    }
}

fn document(uid: &str, revision: u64, gain: f32) -> GraphDocument {
    duplex_document(uid, uid, revision, gain)
}

fn duplex_document(microphone: &str, speaker: &str, revision: u64, gain: f32) -> GraphDocument {
    GraphDocument::new(
        revision,
        vec![
            NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: "input".into(),
                label: None,
                config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: microphone.into(),
                    },
                    output_channels: 2,
                    gain_db: gain,
                }),
            },
            NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: "output".into(),
                label: None,
                config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: speaker.into(),
                    },
                    input_channels: 2,
                }),
            },
        ],
        vec![],
    )
}

/// `graph` with its input wired to its output.
fn wired(graph: &GraphDocument) -> GraphDocument {
    let mut wired = serde_json::to_value(graph).unwrap();
    wired["edges"] = serde_json::json!([{
        "id": "wire",
        "source": {"nodeId": "input", "portId": "audio"},
        "destination": {"nodeId": "output", "portId": "audio"}
    }]);
    serde_json::from_value(wired).unwrap()
}

/// The compiled render plan at `rate` of `graph` as published.
fn compiled(graph: &GraphDocument, rate: u32) -> mixrelay_render::RenderPlan {
    use mixrelay_audio_graph::{CompileContext, CompileLimits, GraphCompiler};
    let execution = GraphCompiler::default()
        .compile(
            graph,
            CompileContext {
                format: format(rate, 256),
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    mixrelay_render::RenderPlan::prepare(&execution).unwrap()
}

/// The engine's control and audio owners over one simulated duplex device. Graph edits go
/// through the production controller, which compiles the processor and reconciles the
/// devices; the test stands in for the audio worker at its block boundary.
struct Engine {
    host: Arc<SimulatedHost>,
    uid: String,
    resources: Arc<MacDevices>,
    exchange: Arc<ProcessorExchange>,
    controller: EngineGraphController,
    processor: Box<BlockProcessor>,
    active: DeviceSet,
}

impl Engine {
    /// Starts like the engine: an empty revision-1 graph, compiled before any device.
    fn start() -> Self {
        let (host, uid, resources) = simulated(48_000);
        let resources = Arc::new(resources);
        let initial = GraphDocument::new(1, vec![], vec![]);
        let processor =
            Box::new(BlockProcessor::new(Arc::new(compiled(&initial, 48_000))).unwrap());
        let exchange = Arc::new(ProcessorExchange::new(processor.plan().revision()));
        let controller = EngineGraphController::new_with_initial_document(
            exchange.clone(),
            EngineGraphControllerConfig {
                clock: std::sync::Arc::new(mixrelay_time::ManualClock::default()),
                format: format(48_000, 256),
                meters: mixrelay_engine_core::default_peak_meter_bank(),
                initial_bindings: vec![],
                endpoint_backend: Arc::new(
                    crate::macos_application_rate_tests::EndpointFixture::new(),
                ),
                required_endpoint_node_id: None,
                processor_catalog: mixrelay_processor_host::ProcessorCatalog::default(),
                graph_resources: resources.clone(),
                runtime_status: resources.clone(),
            },
            initial,
        );
        Self {
            host,
            uid,
            resources,
            exchange,
            controller,
            processor,
            active: DeviceSet::default(),
        }
    }

    /// The duplex graph at `revision` with the device input wired to the output at `gain`.
    fn graph(&self, revision: u64, gain: f32) -> GraphDocument {
        wired(&document(&self.uid, revision, gain))
    }

    /// Submits `graph` through the controller, then installs the processor and device set it
    /// prepared at the next block boundary.
    fn apply(&mut self, graph: &GraphDocument) {
        let response = self
            .controller
            .apply_graph_request(graph.clone().into())
            .unwrap();
        assert_eq!(response.revision, graph.revision);
        assert!(self.boundary(), "the accepted edit was not installed");
        assert_eq!(self.exchange.revisions().active, graph.revision);
        assert_eq!(self.processor.plan().revision(), graph.revision);
    }

    /// The compiled binding ids of the installed processor's sources and sinks.
    fn bound(&self) -> BTreeSet<String> {
        let plan = self.processor.plan();
        plan.source_bindings()
            .iter()
            .chain(plan.sink_bindings())
            .map(|binding| binding.node_id().as_str().to_owned())
            .collect()
    }

    /// The audio worker's block boundary: same-spec recovery, then any pending processor
    /// with the device set prepared for it. True when a processor was installed.
    fn boundary(&mut self) -> bool {
        self.active
            .boundary(&self.resources, &self.exchange, &mut self.processor)
    }
}

/// The graph's devices are published offline under the installed processor's bindings,
/// and transfer silence without any native owner.
fn assert_published_offline(engine: &Engine, graph: &GraphDocument) {
    engine
        .active
        .assert_published_offline(graph, &engine.bound());
}
fn assert_cached_selection(engine: &Engine, graph: &GraphDocument) {
    test_support::assert_cached_selection(&engine.resources, graph, &engine.bound());
}
fn publish_recovery_now(resources: &MacDevices, active: &mut DeviceSet) {
    active.publish_recovery_now(resources);
}
fn wait_failed(active: &DeviceSet) {
    active.wait_failed();
}
fn bindings(active: &DeviceSet) -> [String; 2] {
    active.bindings()
}

/// The recovered owners carry the device's capture through the processor the controller
/// installed for `revision` to the output callback, which presents it at `gain_db`.
fn assert_renders_installed_gain(engine: &mut Engine, revision: u64, gain_db: f32) {
    assert_eq!(engine.exchange.revisions().active, revision);
    let processor = &mut engine.processor;
    assert_eq!(processor.plan().revision(), revision);
    engine
        .active
        .assert_renders_installed_gain(&engine.host, processor, gain_db);
}

fn delete_disconnected_nodes(engine: &mut Engine) {
    let retired = engine.active.retain_lifetime_observations();
    engine.host.unplug(&engine.uid);
    wait_failed(&engine.active);
    let revision = engine.exchange.revisions().active + 1;
    engine.apply(&GraphDocument::new(revision, vec![], vec![]));
    assert!(retired.all_released());
    assert_eq!(engine.host.replug(&engine.uid), DEFAULT_BUFFER);
    publish_recovery_now(&engine.resources, &mut engine.active);
    assert!(engine.active.is_empty());
    assert!(test_support::cache_is_empty(&engine.resources));
    assert!(
        !test_support::has_pending(&engine.resources),
        "device arrival resurrected deleted intent"
    );
}

#[test]
fn control_recovery_publishes_offline_intent_and_restores_the_same_uid_without_graph_edits() {
    let mut engine = Engine::start();
    let original = engine.graph(2, 0.0);
    engine.apply(&original);
    assert_eq!(engine.active.owner_count(), 2);
    let retired = engine.active.retain_lifetime_observations();
    engine.host.unplug(&engine.uid);
    wait_failed(&engine.active);
    publish_recovery_now(&engine.resources, &mut engine.active);
    assert_published_offline(&engine, &original);
    assert!(retired.all_released());

    // An offline graph edit is accepted without preparing any native stream.
    let edited = engine.graph(3, EDITED_GAIN_DB);
    engine.apply(&edited);
    assert_published_offline(&engine, &edited);
    assert_cached_selection(&engine, &edited);

    assert_eq!(engine.host.replug(&engine.uid), DEFAULT_BUFFER);
    // Only control recovery executes here: no graph edit is allowed to create the
    // returning native owners for this assertion.
    wait_until(|| {
        publish_recovery_now(&engine.resources, &mut engine.active);
        engine.active.owner_count() == 2 && engine.active.all_ready()
    });
    assert!(engine.active.offline_is_empty());
    assert!(!engine.boundary(), "recovery published a processor");
    assert_cached_selection(&engine, &edited);
    assert_renders_installed_gain(&mut engine, edited.revision, EDITED_GAIN_DB);
    delete_disconnected_nodes(&mut engine);
}

#[test]
fn device_recovery_retains_graph_owners_and_offline_gain_edits() {
    let mut engine = Engine::start();
    engine.apply(&engine.graph(2, 0.0));
    let [input_id, output_id] = bindings(&engine.active);
    let input = engine.active.owner_identity(&input_id);
    let output = engine.active.owner_identity(&output_id);
    for (revision, gain) in (3..).zip([EDITED_GAIN_DB, 0.0, EDITED_GAIN_DB, 0.0]) {
        let epoch = engine.active.epoch(&input_id);
        engine.host.unplug(&engine.uid);
        wait_failed(&engine.active);
        // Changing gain while the device is absent must not try to prepare a
        // new native stream or block an otherwise valid graph edit.
        engine.apply(&engine.graph(revision, gain));
        assert_eq!(engine.active.owner_identity(&input_id), input);
        assert_eq!(engine.active.owner_identity(&output_id), output);
        assert_eq!(engine.host.replug(&engine.uid), DEFAULT_BUFFER);
        // The same owners reconnect on their own workers, with a new PCM epoch.
        wait_until(|| engine.active.none_failed() && engine.active.epoch(&input_id) > epoch);
        assert_renders_installed_gain(&mut engine, revision, gain);
    }
    delete_disconnected_nodes(&mut engine);
}
