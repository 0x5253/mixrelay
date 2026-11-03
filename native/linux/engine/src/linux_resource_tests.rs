use super::*;
use applications::tests::{FixtureProcess, ObservedDriver};
use mixrelay_audio_graph::{ApplicationSelector, NodeConfig, NodeSpec};
use mixrelay_audio_io::{ChannelLayout, SourcePlaybackPolicy, StreamSpec};
use std::collections::BTreeSet;

struct Resources {
    backend: Arc<LinuxVirtualEndpointBackend>,
    endpoints: Box<EndpointSet>,
    captures: Box<PipeWireCaptureSet>,
    devices: Box<DeviceStreams>,
}

impl Resources {
    fn new() -> Self {
        Self {
            // Functional polling publisher, not a native-cycle timing measurement.
            backend: LinuxVirtualEndpointBackend::new_standalone_device_publishers(),
            endpoints: Box::default(),
            captures: Box::default(),
            devices: Box::default(),
        }
    }

    fn apply(&mut self, document: &GraphDocument) -> usize {
        self.backend
            .reconcile(document, mixrelay_engine_core::PlaybackBindings::Preserve)
            .unwrap();
        let staged = resource_serials().len();
        let mut result = Ok(());
        let allocations = allocation_counter::measure(|| {
            result = self.backend.activate_pending(
                document.revision,
                &mut self.endpoints,
                &mut self.captures,
                &mut self.devices,
            );
        });
        result.unwrap();
        assert_eq!(allocations.count_total, 0);
        assert_eq!(
            allocations.count_current, 0,
            "native activation must not reclaim either"
        );
        self.backend.reclaim_retired();
        staged
    }

    fn assert_pcm(&mut self, node: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut pcm = [0.0; 256];
            let outcome = self.captures.read(node, &mut pcm);
            if outcome.frames > 0 && pcm.iter().any(|sample| sample.abs() > 0.05) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "native PCM did not arrive for {node}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn resource_serials() -> BTreeSet<String> {
    mixrelay_linux_audio_io::targets()
        .unwrap()
        .into_iter()
        .filter(|target| {
            target.process_id == Some(std::process::id())
                && target.node_name.as_ref().is_some_and(|name| {
                    name.starts_with("io.mixrelay.keep.capture.")
                        || name.starts_with("io.mixrelay.add.capture.")
                        || name == "io.mixrelay.keep.device-input"
                        || name == "io.mixrelay.add.device-output"
                })
        })
        .map(|target| target.serial)
        .collect()
}

fn document(revision: u64, sources: &[(&str, u32)]) -> GraphDocument {
    GraphDocument::new(
        revision,
        sources
            .iter()
            .map(|(id, process_id)| NodeSpec {
                id: (*id).into(),
                label: None,
                control: mixrelay_audio_graph::NodeControl::default(),
                config: NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
                    selector: ApplicationSelector::ProcessId {
                        process_id: *process_id,
                        include_descendants: false,
                    },
                    stream: StreamSpec {
                        channels: 2,
                        channel_layout: ChannelLayout::Stereo,
                    },
                    source_playback: SourcePlaybackPolicy::Preserve,
                    forwarding_endpoint: None,
                    gain_db: 0.0,
                    requires_reselection: false,
                }),
            })
            .collect(),
        Vec::new(),
    )
}

#[test]
#[cfg(feature = "native-lab")]
fn native_capture_graph_edits_retain_unaffected_streams() {
    let path = std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    assert!(path.is_file() && path.is_absolute());
    let _driver = ObservedDriver::new();
    let first = FixtureProcess::start(&path, &[]);
    let second = FixtureProcess::start(&path, &[]);
    let deadline = Instant::now() + Duration::from_secs(3);
    while ![first.id(), second.id()].iter().all(|pid| {
        mixrelay_linux_audio_io::targets()
            .unwrap()
            .iter()
            .any(|target| target.process_id == Some(*pid))
    }) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let mut resources = Resources::new();
    resources.apply(&document(1, &[("keep", first.id())]));
    resources.assert_pcm("keep");
    let initial = resource_serials();
    assert_eq!(initial.len(), 1);
    let staged_add = resources.apply(&document(2, &[("keep", first.id()), ("add", second.id())]));
    resources.assert_pcm("keep");
    resources.assert_pcm("add");
    let added = resource_serials();
    let staged_remove = resources.apply(&document(3, &[("keep", first.id())]));
    resources.assert_pcm("keep");
    let removed = resource_serials();
    println!(
        "initial={initial:?}; after_add={added:?}; after_remove={removed:?}; staged_add_count={staged_add}; staged_remove_count={staged_remove}"
    );
    resources.apply(&document(4, &[]));
    assert!(resource_serials().is_empty());
    assert_eq!(staged_add, 2, "only the newly referenced source may open");
    assert_eq!(staged_remove, 2, "removal must open no replacement source");
    assert!(
        initial.is_subset(&added),
        "unaffected capture reopened during addition"
    );
    assert_eq!(
        initial, removed,
        "unaffected capture reopened during removal"
    );
}

fn device_document(revision: u64, input: bool, output: bool) -> GraphDocument {
    use mixrelay_audio_graph::{AudioDeviceInputConfig, AudioDeviceOutputConfig};
    use mixrelay_audio_io::DeviceSelector;
    let mut nodes = Vec::new();
    if input {
        nodes.push(NodeSpec {
            id: "keep".into(),
            label: None,
            control: mixrelay_audio_graph::NodeControl::default(),
            config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::PersistentUid {
                    uid: "io.mixrelay.lab-device-source".into(),
                },
                output_channels: 32,
                gain_db: 0.0,
            }),
        });
    }
    if output {
        nodes.push(NodeSpec {
            id: "add".into(),
            label: None,
            control: mixrelay_audio_graph::NodeControl::default(),
            config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: DeviceSelector::PersistentUid {
                    uid: "io.mixrelay.lab-device-sink".into(),
                },
                input_channels: 32,
            }),
        });
    }
    GraphDocument::new(revision, nodes, Vec::new())
}

fn pump_device_pcm(devices: &mut DeviceStreams) {
    let mut samples = vec![0.0; 128 * 32];
    let mut peaks = [0.0_f32; 32];
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        let frames = devices.read("keep", &mut samples).frames as usize;
        for frame in samples[..frames * 32].chunks_exact(32) {
            for (peak, sample) in peaks.iter_mut().zip(frame) {
                *peak = peak.max(sample.abs());
            }
        }
        devices.write("add", &samples[..frames * 32]);
        thread::sleep(Duration::from_millis(1));
    }
    for (channel, peak) in peaks.iter().enumerate() {
        assert!((*peak - f32::from(u16::try_from(channel + 1).unwrap()) / 1024.0).abs() < 0.00001);
    }
}

fn device_fixture() -> (ObservedDriver, FixtureProcess) {
    let path = std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_DEVICE_FIXTURE").unwrap());
    assert!(path.is_absolute() && path.is_file());
    let driver = ObservedDriver::new();
    let fixture = FixtureProcess::start(&path, &["32", "32"]);
    wait_device_source(true);
    (driver, fixture)
}

fn wait_device_source(present: bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while mixrelay_linux_audio_io::targets()
        .unwrap()
        .iter()
        .any(|target| target.node_name.as_deref() == Some("io.mixrelay.lab-device-source"))
        != present
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
}

fn rejected_device_edit(resources: &mut Resources) {
    let original = resource_serials();
    let mut invalid = device_document(99, true, true);
    if let NodeConfig::AudioDeviceOutput(output) = &mut invalid.nodes[1].config {
        output.selector = mixrelay_audio_io::DeviceSelector::PersistentUid {
            uid: "missing-device".into(),
        };
    }
    assert!(
        resources
            .backend
            .reconcile(&invalid, mixrelay_engine_core::PlaybackBindings::Preserve)
            .is_err()
    );
    assert!(resources.backend.pending.load(Ordering::Acquire).is_null());
    assert_eq!(resource_serials(), original);
    pump_device_pcm(&mut resources.devices);
}

#[test]
#[cfg(feature = "native-lab")]
fn native_device_graph_edits_retain_unaffected_streams() {
    let (_driver, fixture) = device_fixture();
    let mut resources = Resources::new();
    resources.apply(&device_document(1, true, false));
    pump_device_pcm(&mut resources.devices);
    let initial = resource_serials();
    assert_eq!(initial.len(), 1);
    rejected_device_edit(&mut resources);
    let staged_add = resources.apply(&device_document(2, true, true));
    pump_device_pcm(&mut resources.devices);
    let added = resource_serials();
    let staged_remove = resources.apply(&device_document(3, false, true));
    let output_only = resource_serials();
    let staged_readd = resources.apply(&device_document(4, true, true));
    pump_device_pcm(&mut resources.devices);
    let readded = resource_serials();
    println!(
        "device_initial={initial:?}; added={added:?}; output_only={output_only:?}; readded={readded:?}; staged={staged_add}/{staged_remove}/{staged_readd}"
    );
    resources.apply(&device_document(5, false, false));
    assert!(resource_serials().is_empty());
    let report = fixture.report();
    println!("device_sink={report}");
    assert_eq!(report["invalid"], 0);
    assert!(report["active"].as_u64().unwrap() > 1_000);
    for (channel, peak) in report["peaks"].as_array().unwrap().iter().enumerate() {
        assert!(
            (peak.as_f64().unwrap() - f64::from(u16::try_from(channel + 1).unwrap()) / 1024.0)
                .abs()
                < 0.00001
        );
    }
    assert_eq!((staged_add, staged_remove, staged_readd), (2, 2, 2));
    assert!(initial.is_subset(&added));
    assert!(output_only.is_subset(&added) && output_only.is_subset(&readded));
    assert!(
        initial.is_disjoint(&readded),
        "removed input must acquire a fresh native owner"
    );
}

fn disconnect_source(resources: &mut Resources, fixture: &mut FixtureProcess) -> DeviceWatch {
    fixture.signal(libc::SIGUSR2);
    wait_device_source(false);
    let watch = resources
        .backend
        .device_watch
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        resources.devices.take_report();
        if !watch.disconnected().inputs.is_empty() {
            return watch;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
}

fn prepare_returned_device(
    resources: &mut Resources,
    fixture: &mut FixtureProcess,
) -> DeviceStreams {
    let watch = disconnect_source(resources, fixture);
    fixture.signal(libc::SIGUSR2);
    wait_device_source(true);
    DeviceStreams::recover(&watch, &AtomicBool::new(false)).unwrap()
}

fn activate_device_recovery(resources: &mut Resources, mut recovery: DeviceStreams) {
    assert_eq!(resources.devices.generation, recovery.generation);
    let allocations =
        allocation_counter::measure(|| resources.devices.replace_disconnected(&mut recovery));
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.count_current, 0);
    // The replaced native owner drops only after the measured audio boundary.
}

#[test]
#[cfg(feature = "native-lab")]
fn native_device_recovery_is_fenced_by_acquisition() {
    let (_driver, mut fixture) = device_fixture();
    let mut resources = Resources::new();
    resources.apply(&device_document(1, true, false));
    let old = resource_serials();
    let recovery = prepare_returned_device(&mut resources, &mut fixture);
    let recovered = resource_serials()
        .difference(&old)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(recovered.len(), 1);
    resources.apply(&device_document(2, true, true));
    activate_device_recovery(&mut resources, recovery);
    assert!(recovered.is_subset(&resource_serials()));
    assert!(old.is_disjoint(&resource_serials()));
    pump_device_pcm(&mut resources.devices);

    let before_retry = resource_serials();
    let stale = prepare_returned_device(&mut resources, &mut fixture);
    let stale_native = resource_serials()
        .difference(&before_retry)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(stale_native.len(), 1);
    resources.apply(&device_document(3, false, true));
    resources.apply(&device_document(4, true, true));
    let before = resource_serials();
    let watch = disconnect_source(&mut resources, &mut fixture);
    let current_owner = resources.devices.input_owner("keep").unwrap();
    activate_device_recovery(&mut resources, stale);
    let after = resource_serials();
    assert_eq!(before.len(), 3);
    // PipeWire unpublishes target-bound nodes on disconnection. The Rust/native
    // allocation remains owned; compare that identity, not registry presence.
    assert_eq!(resources.devices.input_owner("keep"), Some(current_owner));
    assert!(
        stale_native.is_disjoint(&after),
        "late recovery replaced a different acquisition"
    );
    fixture.signal(libc::SIGUSR2);
    wait_device_source(true);
    // The stale disconnected stream must be gone, not installed into the new lease.
    let actual_recovery = DeviceStreams::recover(&watch, &AtomicBool::new(false)).unwrap();
    activate_device_recovery(&mut resources, actual_recovery);
    assert_ne!(resources.devices.input_owner("keep"), Some(current_owner));
    pump_device_pcm(&mut resources.devices);
    resources.apply(&device_document(5, false, false));
    assert!(resource_serials().is_empty());
}
