#[cfg(feature = "native-lab")]
use super::super::applications::tests::FixtureProcess;
use super::super::*;
use super::*;
#[cfg(feature = "native-lab")]
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioEndpoint, ChannelMapping, EdgeSpec, FROM_OS_PORT_ID, NodeConfig,
    NodeSpec, TO_OS_PORT_ID, VirtualEndpointConfig, VirtualEndpointReference,
};
use mixrelay_audio_graph::{ChannelLayout, StreamSpec};
#[cfg(feature = "native-lab")]
use mixrelay_audio_io::DeviceSelector;
#[cfg(feature = "native-lab")]
use mixrelay_control_protocol::{
    ErrorCode, GraphCatalogAction, GraphCatalogRequest, GraphLayout, GraphLayoutRequest,
    GraphNodePosition, NodeControlRequest,
};

const A: &str = "31ab4e5a-44e0-4bab-89dd-906e07751ea1";
const B: &str = "31ab4e5a-44e0-4bab-89dd-906e07751ea2";

fn device_spec(id: &str) -> VirtualEndpointSpec {
    let stream = StreamSpec {
        channels: 32,
        channel_layout: ChannelLayout::Discrete,
    };
    VirtualEndpointSpec {
        id: id.into(),
        display_name: format!("Bus {id}"),
        render: Some(stream),
        capture: Some(stream),
        supported_sample_rates: vec![48_000],
        default_sample_rate: 48_000,
    }
}

/// Owns only the two explicitly created native test devices, outside engine lifetime.
#[cfg(feature = "native-lab")]
struct DriverFixture(mixrelay_driver_adapters::DriverClient, Vec<&'static str>);

#[cfg(feature = "native-lab")]
impl DriverFixture {
    fn new() -> Self {
        assert!(std::env::var_os("MIXRELAY_LAB_DEVICE_FIXTURE").is_some());
        let client = mixrelay_driver_adapters::DriverClient::new().unwrap();
        let mut snapshot = client.snapshot_blocking().unwrap();
        assert_eq!(snapshot.state, mixrelay_driver_core::DriverState::Ready);
        for id in [A, B] {
            assert!(
                !snapshot
                    .settings
                    .devices
                    .iter()
                    .any(|device| device.spec.id.as_str() == id),
                "native Driver fixture IDs must start absent"
            );
        }
        let mut fixture = Self(client, Vec::new());
        for id in [A, B] {
            snapshot = fixture
                .0
                .mutate_blocking(&mixrelay_driver_core::DriverCommand::Create {
                    expected_revision: snapshot.revision,
                    spec: device_spec(id),
                })
                .unwrap();
            fixture.1.push(id);
        }
        fixture
    }
}

#[cfg(feature = "native-lab")]
impl Drop for DriverFixture {
    fn drop(&mut self) {
        for id in &self.1 {
            let result = self.0.snapshot_blocking().and_then(|snapshot| {
                self.0
                    .mutate_blocking(&mixrelay_driver_core::DriverCommand::Remove {
                        expected_revision: snapshot.revision,
                        endpoint_id: (*id).into(),
                    })
            });
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

#[cfg(feature = "native-lab")]
fn configure_driver(spec: VirtualEndpointSpec) {
    let client = mixrelay_driver_adapters::DriverClient::new().unwrap();
    let snapshot = client.snapshot_blocking().unwrap();
    client
        .mutate_blocking(&mixrelay_driver_core::DriverCommand::Configure {
            expected_revision: snapshot.revision,
            spec,
        })
        .unwrap();
}

#[test]
fn identities_are_not_aliased_by_punctuation_labels_or_graph_names() {
    assert_ne!(native_name(&"a:b".into()), native_name(&"a_b".into()));
    let mut spec = device_spec(A);
    let names = native_names(&spec);
    spec.display_name = "Renamed".into();
    assert_eq!(names, native_names(&spec));
    assert!(specs(vec![("one".into(), spec.clone()), ("two".into(), spec)]).is_err());
    assert!(
        specs(vec![
            ("one".into(), device_spec(A)),
            ("one".into(), device_spec(B))
        ])
        .is_err()
    );
    let backend = test_backend();
    assert!(backend.capabilities().supports_dynamic_creation);
    backend.prepare_graph_attachments(&[]).unwrap();
}

#[test]
fn empty_inventory_needs_no_server_and_unavailable_reads_are_silent() {
    let mut set = EndpointSet::prepare(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &EndpointSpecs::new(),
        &EndpointSpecs::new(),
        render_format(),
        render_format(),
    )
    .unwrap();
    assert!(set.is_empty() && set.ready() && set.can_retain_from(&EndpointSet::default()));
    let mut samples = [1.0; 64];
    assert_eq!(set.read("missing", &mut samples).frames, 0);
    assert!(samples.iter().all(|value| value.to_bits() == 0));
    assert_eq!(set.write("missing", &samples).frames, 0);
}

#[cfg(feature = "native-lab")]
struct Session {
    // Drain the production callback before dropping its controller/native owners.
    _worker: mixrelay_engine_host::AudioWorker,
    controller: Arc<EngineGraphController>,
    exchange: Arc<ProcessorExchange>,
    backend: Arc<LinuxVirtualEndpointBackend>,
}

#[cfg(feature = "native-lab")]
impl Session {
    fn new() -> Self {
        let document = GraphDocument::new(1, Vec::new(), Vec::new());
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
            create_initial_processor(&document, meters.clone(), render_format()).unwrap();
        let exchange = Arc::new(ProcessorExchange::new(1));
        let controller = Arc::new(
            EngineGraphController::new_with_initial_document(
                exchange.clone(),
                EngineGraphControllerConfig {
                    clock: std::sync::Arc::new(mixrelay_time::StdClock::new()),
                    format: render_format(),
                    meters,
                    initial_bindings: Vec::new(),
                    endpoint_backend: backend.clone(),
                    required_endpoint_node_id: None,

                    graph_resources: backend.clone(),
                    runtime_status: status.clone(),

                    documents: Arc::new(mixrelay_engine_host::document_files::DiskDocuments),
                    incarnation: mixrelay_engine_host::identity::new_incarnation(),
                    ..config
                },
                document,
            )
            .with_graph_catalog_path(
                mixrelay_engine_host::persistence::config_directory()
                    .unwrap()
                    .join("graphs.json"),
            ),
        );
        let worker_status = status.clone();
        let worker_exchange = exchange.clone();
        let worker_backend = backend.clone();
        let thread = thread::spawn(move || {
            run_audio_worker(processor, worker_exchange, &worker_backend, &worker_status)
                .expect("native PipeWire worker failed");
        });
        Self {
            _worker: mixrelay_engine_host::AudioWorker::new(status, thread),
            controller,
            exchange,
            backend,
        }
    }
    fn settle(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.exchange.revisions().active != self.exchange.revisions().submitted {
            assert!(
                Instant::now() < deadline,
                "native graph publication timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
        self.exchange.reclaim_retired();
        self.backend.reclaim_retired();
    }
    fn apply(&self, mut document: GraphDocument) {
        document.revision = self.controller.graph().revision + 1;
        self.controller
            .apply_graph_request(document.into())
            .unwrap();
        self.settle();
    }
    fn catalog(&self, action: GraphCatalogAction) -> String {
        let catalog = self.controller.graph_catalog().unwrap();
        let next = self
            .controller
            .mutate_graph_catalog(GraphCatalogRequest {
                expected_revision: self.controller.graph().revision,
                expected_catalog_revision: catalog.catalog_revision,
                action,
            })
            .unwrap();
        self.settle();
        next.active_id
    }

    fn control(&self, graph: &str, node: &str, mut request: NodeControlRequest) {
        request.expected_revision = self.controller.graph().revision;
        self.controller
            .update_node_control(graph, node, &request)
            .unwrap();
        self.settle();
    }
}

#[cfg(feature = "native-lab")]
fn reference(id: &str, node: &str) -> NodeSpec {
    NodeSpec {
        id: node.into(),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::VirtualEndpoint(VirtualEndpointConfig {
            spec: device_spec(id),
            reference: VirtualEndpointReference {
                render: true,
                capture: true,
            },
            gain_db: 0.0,
        }),
    }
}

#[cfg(feature = "native-lab")]
fn cable(source: (&str, &str), destination: (&str, &str)) -> EdgeSpec {
    EdgeSpec {
        id: "cable".into(),
        source: AudioEndpoint {
            node_id: source.0.into(),
            port_id: source.1.into(),
        },
        destination: AudioEndpoint {
            node_id: destination.0.into(),
            port_id: destination.1.into(),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}

#[cfg(feature = "native-lab")]
fn input_graph() -> GraphDocument {
    let input = NodeSpec {
        id: "input".into(),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::PersistentUid {
                uid: "io.mixrelay.lab-device-source".into(),
            },
            output_channels: 32,
            gain_db: 0.0,
        }),
    };
    GraphDocument::new(
        2,
        vec![input, reference(A, "a")],
        vec![cable(("input", "audio"), ("a", TO_OS_PORT_ID))],
    )
}

#[cfg(feature = "native-lab")]
fn forwarding_graph() -> GraphDocument {
    GraphDocument::new(
        3,
        vec![reference(A, "a"), reference(B, "b")],
        vec![cable(("a", FROM_OS_PORT_ID), ("b", TO_OS_PORT_ID))],
    )
}

#[cfg(feature = "native-lab")]
fn inventory() -> BTreeMap<String, String> {
    let names: Vec<_> = [A, B]
        .into_iter()
        .flat_map(|id| native_names(&device_spec(id)))
        .collect();
    mixrelay_linux_audio_io::targets()
        .unwrap()
        .into_iter()
        .filter_map(|target| {
            let name = target.node_name?;
            names.contains(&name).then_some((name, target.serial))
        })
        .collect()
}

#[cfg(feature = "native-lab")]
fn save_native_formats() {
    let output = std::process::Command::new("pw-dump").output().unwrap();
    assert!(output.status.success(), "native format inventory failed");
    let inventory: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let fixture =
        std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let file =
        std::fs::File::create_new(fixture.with_file_name("native-endpoint-formats.json")).unwrap();
    serde_json::to_writer(file, &inventory).unwrap();
    let nodes: Vec<_> = inventory
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| {
            node.pointer("/info/props/node.name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.starts_with("io.mixrelay.endpoint."))
        })
        .collect();
    assert_eq!(nodes.len(), 4);
    for node in nodes {
        let id = node["id"].as_u64().unwrap().to_string();
        let format = std::process::Command::new("pw-cli")
            .args(["enum-params", &id, "Format"])
            .output()
            .unwrap();
        assert!(format.status.success());
        let mut file =
            std::fs::File::create_new(fixture.with_file_name(format!("native-format-{id}.txt")))
                .unwrap();
        std::io::Write::write_all(&mut file, &format.stdout).unwrap();
        assert_eq!(
            node.pointer("/info/props/audio.channels")
                .and_then(serde_json::Value::as_u64),
            Some(32)
        );
        let formats = node
            .pointer("/info/params/Format")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0]["channels"], 32);
        assert_eq!(formats[0]["rate"], 48_000);
    }
}

#[cfg(feature = "native-lab")]
fn fixture() -> FixtureProcess {
    let path = std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_DEVICE_FIXTURE").unwrap());
    assert!(path.is_absolute() && path.is_file());
    let fixture = FixtureProcess::start(&path, &["32", "32"]);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !mixrelay_linux_audio_io::targets()
        .unwrap()
        .iter()
        .any(|target| target.node_name.as_deref() == Some("io.mixrelay.lab-device-source"))
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    fixture
}

#[cfg(feature = "native-lab")]
fn reader(id: &str) -> NativeStream {
    let name = format!("{}.capture", native_name(&id.into()));
    let serial = inventory()[&name].clone();
    capture_target(&serial)
}

#[cfg(feature = "native-lab")]
fn capture_target(target: &str) -> NativeStream {
    let stream = NativeStream::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        "lab.capture-reader",
        "Owned capture observer",
        StreamRole::DeviceInput(target),
        32,
        super::super::native_format(render_format()),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !stream.ready() || !stream.connected() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    stream
}

#[cfg(feature = "native-lab")]
fn assert_pcm(reader: &mut NativeStream) {
    assert_signal(reader, 0.0, 1.0);
}

#[cfg(feature = "native-lab")]
fn assert_signal(reader: &mut NativeStream, offset: f32, gain: f32) {
    let mut pcm = vec![0.0; 128 * 32];
    let mut matching = 0;
    let mut best_match = 0;
    let mut total_frames = 0;
    let mut active_frames = 0;
    let mut empty_reads = 0;
    let mut observed_peaks = [0.0f32; 32];
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let frames = reader.read(&mut pcm) as usize;
        total_frames += frames;
        empty_reads += usize::from(frames == 0);
        let mut minimums = [f32::INFINITY; 32];
        let mut maximums = [f32::NEG_INFINITY; 32];
        for frame in pcm[..frames * 32].chunks_exact(32) {
            active_frames += usize::from(frame.iter().any(|sample| *sample != 0.0));
            for (channel, sample) in frame.iter().enumerate() {
                assert!(sample.is_finite());
                minimums[channel] = minimums[channel].min(*sample);
                maximums[channel] = maximums[channel].max(*sample);
                observed_peaks[channel] = observed_peaks[channel].max(sample.abs());
            }
        }
        if minimums
            .iter()
            .zip(maximums)
            .enumerate()
            .all(|(channel, (min, max))| {
                let amplitude = gain * f32::from(u16::try_from(channel + 1).unwrap()) / 1024.0;
                (*min - (offset - amplitude)).abs() < 0.00001
                    && (max - (offset + amplitude)).abs() < 0.00001
            })
        {
            matching += 1;
            best_match = best_match.max(matching);
        } else if frames != 0 {
            matching = 0;
        }
        if matching == 8 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native PCM mismatch: offset={offset} gain={gain} min={minimums:?} max={maximums:?}; \
             window_frames={total_frames} active_frames={active_frames} empty_reads={empty_reads} \
             best_match={best_match} peaks={observed_peaks:?}; ready={} connected={} \
             format={} quantum={} report={:?}",
            reader.ready(),
            reader.connected(),
            reader.negotiated_format(),
            reader.callback_quantum(),
            reader.take_report(),
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[path = "linux_endpoint_signal_tests.rs"]
mod signal;

#[path = "linux_endpoint_ownership_tests.rs"]
mod ownership;

#[path = "linux_endpoint_rename_tests.rs"]
mod rename;

#[path = "linux_endpoint_settings_native_tests.rs"]
mod settings_native;

#[test]
#[cfg(feature = "native-lab")]
fn native_device_input_and_first_graph_export_pcm() {
    let _driver_devices = DriverFixture::new();
    let session = Session::new();
    let _fixture = fixture();
    let mut direct = capture_target("io.mixrelay.lab-device-source");
    assert_pcm(&mut direct);
    println!("stage=direct_device_capture; channels=32; signal=passed");
    drop(direct);

    session.apply(input_graph());
    let mut exported = reader(A);
    assert_pcm(&mut exported);
    println!("stage=first_graph_export; channels=32; signal=passed");
}

#[cfg(feature = "native-lab")]
struct NativeCatalogSnapshot {
    graph_a: String,
    graph_b: String,
    document_a: GraphDocument,
    document_b: GraphDocument,
    layout_a: GraphLayout,
    layout_b: GraphLayout,
    first: BTreeMap<String, String>,
}

#[cfg(feature = "native-lab")]
fn save_native_graphs(session: &Session, catalog_path: &std::path::Path) -> NativeCatalogSnapshot {
    let graph_a = session.controller.graph_catalog().unwrap().active_id;
    session.apply(input_graph());
    let document_a = session.controller.graph();
    session.catalog(GraphCatalogAction::Rename {
        id: graph_a.clone(),
        name: "Input 32".into(),
    });
    let layout_a = GraphLayout {
        positions: [("input".into(), GraphNodePosition { x: 120.0, y: 80.0 })].into(),
        ..GraphLayout::default()
    };
    session
        .controller
        .update_graph_layout(
            &graph_a,
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout_a.clone(),
            },
        )
        .unwrap();
    let first = inventory();
    assert_eq!(first.len(), 4);
    let graph_b = session.catalog(GraphCatalogAction::Create {
        name: "Forwarding".into(),
    });
    assert_eq!(inventory(), first, "creation must not change native owners");
    session.apply(forwarding_graph());
    let document_b = session.controller.graph();
    let layout_b = GraphLayout {
        positions: [(
            "b::host-sink".into(),
            GraphNodePosition { x: 480.0, y: 240.0 },
        )]
        .into(),
        ..GraphLayout::default()
    };
    session
        .controller
        .update_graph_layout(
            &graph_b,
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: layout_b.clone(),
            },
        )
        .unwrap();
    let catalog = session.controller.graph_catalog().unwrap();
    let stale = GraphCatalogRequest {
        expected_revision: document_b.revision - 1,
        expected_catalog_revision: catalog.catalog_revision,
        action: GraphCatalogAction::Rename {
            id: graph_b.clone(),
            name: "Rejected stale name".into(),
        },
    };
    let saved = std::fs::read(catalog_path).unwrap();
    assert_eq!(
        session
            .controller
            .mutate_graph_catalog(stale)
            .unwrap_err()
            .code,
        ErrorCode::GraphRevisionStale
    );
    assert_eq!(std::fs::read(catalog_path).unwrap(), saved);
    NativeCatalogSnapshot {
        graph_a,
        graph_b,
        document_a,
        document_b,
        layout_a,
        layout_b,
        first,
    }
}

#[cfg(feature = "native-lab")]
fn exercise_native_catalog_views(
    session: &Session,
    catalog_path: &std::path::Path,
    expected: &NativeCatalogSnapshot,
) -> Vec<u8> {
    let both = inventory();
    assert_eq!(both.len(), 4);
    assert!(
        expected
            .first
            .iter()
            .all(|(name, serial)| both.get(name) == Some(serial))
    );
    let mut capture = reader(B);
    assert_pcm(&mut capture);
    save_native_formats();
    let revision = session.exchange.revisions();
    for id in [
        &expected.graph_a,
        &expected.graph_b,
        &expected.graph_a,
        &expected.graph_b,
    ] {
        session.catalog(GraphCatalogAction::Select { id: id.clone() });
        assert_eq!(session.exchange.revisions(), revision);
        assert_eq!(inventory(), both);
        assert_pcm(&mut capture);
    }
    let settings = session.backend.settings().unwrap();
    assert_eq!(settings.devices.len(), 2);
    assert!(
        settings
            .devices
            .iter()
            .flat_map(|device| &device.native_device_ids)
            .all(|name| both.contains_key(name))
    );
    std::fs::read(catalog_path).unwrap()
}

#[cfg(feature = "native-lab")]
fn assert_recalled_catalog(session: &Session, expected: &NativeCatalogSnapshot) {
    session.settle();
    let restored = session.controller.graph_catalog().unwrap();
    assert_eq!(restored.active_id, expected.graph_b);
    assert_eq!(restored.graphs.len(), 2);
    assert_eq!(restored.graphs[0].name, "Input 32");
    assert_eq!(restored.graphs[1].name, "Forwarding");
    assert_eq!(restored.layout, expected.layout_b);
    assert_eq!(
        session
            .controller
            .graph_layout(&expected.graph_a)
            .unwrap()
            .layout,
        expected.layout_a
    );
    let restored_a = session
        .controller
        .graph_catalog_document(&expected.graph_a)
        .unwrap();
    assert_eq!(restored_a.nodes, expected.document_a.nodes);
    assert_eq!(restored_a.edges, expected.document_a.edges);
    assert_eq!(restored.document.nodes, expected.document_b.nodes);
    assert_eq!(restored.document.edges, expected.document_b.edges);
    assert_eq!(session.controller.graph().nodes, expected.document_b.nodes);
    let published = session.controller.graph_status();
    assert_eq!(published.active_revision, published.submitted_revision);
    assert_eq!(inventory().len(), 4);
    let mut capture = reader(B);
    assert_pcm(&mut capture);
}

#[cfg(feature = "native-lab")]
fn delete_native_graphs(session: &Session, expected: &NativeCatalogSnapshot) {
    session.catalog(GraphCatalogAction::Delete {
        id: expected.graph_b.clone(),
    });
    let surviving = inventory();
    assert_eq!(surviving.len(), expected.first.len());
    assert!(
        expected
            .first
            .keys()
            .all(|name| surviving.contains_key(name))
    );
    let mut remaining = reader(A);
    assert_pcm(&mut remaining);
    drop(remaining);
    session.catalog(GraphCatalogAction::Delete {
        id: expected.graph_a.clone(),
    });
    assert_eq!(
        inventory(),
        surviving,
        "graph deletion must not remove Driver devices"
    );
    let empty = session.controller.graph_catalog().unwrap();
    assert_eq!(empty.graphs.len(), 1);
    assert_ne!(empty.active_id, expected.graph_a);
    assert_ne!(empty.active_id, expected.graph_b);
    assert!(empty.document.nodes.is_empty() && empty.document.edges.is_empty());
    let health = session.controller.status();
    assert!(health.engine_power.unwrap().enabled);
    let audio = health.audio_session.unwrap();
    assert!(matches!(
        audio.processing_state,
        mixrelay_audio_io::process::ProcessingState::Idle
            | mixrelay_audio_io::process::ProcessingState::Running
    ));
    assert_ne!(
        audio.issue,
        Some(mixrelay_audio_io::session::SessionIssue::ProcessingFailed)
    );
    let published = session.controller.graph_status();
    assert_eq!(published.active_revision, published.submitted_revision);
}

#[test]
#[cfg(feature = "native-lab")]
fn native_independent_endpoints_share_pcm_across_graphs() {
    let _driver_devices = DriverFixture::new();
    let config = std::path::PathBuf::from(
        std::env::var_os("MIXRELAY_CONFIG_DIR")
            .expect("explicit private config directory required"),
    );
    assert!(config.is_absolute() && config.is_dir());
    let catalog_path = config.join("graphs.json");
    assert!(
        !catalog_path.exists(),
        "native catalog fixture must start empty"
    );
    let session = Session::new();
    let _fixture = fixture();
    let expected = save_native_graphs(&session, &catalog_path);
    let saved = exercise_native_catalog_views(&session, &catalog_path, &expected);
    drop(session);
    assert_eq!(
        inventory().len(),
        4,
        "Driver devices must survive engine shutdown"
    );
    let playback = signal::Playback::new(A, 0.125);
    let mut capture = reader(A);
    assert_signal(&mut capture, 0.125, 0.0);
    drop(capture);
    drop(playback);
    assert_eq!(std::fs::read(&catalog_path).unwrap(), saved);
    let session = Session::new();
    assert_recalled_catalog(&session, &expected);
    delete_native_graphs(&session, &expected);
    println!(
        "driver_devices=2; duplex_nodes=4; mapped_channels=32; view_switch_reopens=0; driver_pcm_after_quit=passed; reference_removal_preserves_driver=passed"
    );
}
