use super::{open::OpenPolicy, open::target_serial, *};
use crate::runtime::render_format;
use mixrelay_audio_graph::{GraphDocument, NodeConfig};
use mixrelay_linux_pipewire::StreamRole;

#[test]
fn target_serial_accepts_u64_identity_and_rejects_invalid_or_overflowing_text() {
    for (serial, expected) in [
        ("123", Ok(123)),
        ("18446744073709551615", Ok(u64::MAX)),
        ("not-a-serial", Err("invalid PipeWire serial")),
        ("18446744073709551616", Err("invalid PipeWire serial")),
    ] {
        let target = mixrelay_linux_audio_io::PipeWireTarget {
            object_id: 1,
            serial: serial.into(),
            node_name: None,
            process_id: None,
            application_id: None,
            executable: None,
            icon_name: None,
            display_name: String::new(),
            media_class: String::new(),
            channels: 2,
        };
        assert_eq!(target_serial(&target), expected.map_err(str::to_owned));
    }
}

#[test]
fn only_recovery_allows_partial_open_and_carries_cancellation() {
    assert!(!OpenPolicy::Initial.allows_partial());
    assert!(OpenPolicy::Initial.check_cancelled().is_ok());
    assert!(!OpenPolicy::StandalonePublisher.allows_partial());
    assert!(OpenPolicy::StandalonePublisher.automatic_outputs());
    let stopping = AtomicBool::new(false);
    let recovery = OpenPolicy::Recovery(&stopping);
    let standalone_recovery = OpenPolicy::StandaloneRecovery(&stopping);
    assert_eq!(
        recovery.output_role("sink"),
        StreamRole::DeviceOutput("sink")
    );
    assert_eq!(
        standalone_recovery.output_role("sink"),
        StreamRole::AutomaticDeviceOutput("sink")
    );
    assert!(recovery.allows_partial());
    assert!(recovery.check_cancelled().is_ok());
    assert!(standalone_recovery.allows_partial());
    assert!(standalone_recovery.automatic_outputs());
    assert!(standalone_recovery.check_cancelled().is_ok());
    stopping.store(true, Ordering::Release);
    assert!(recovery.check_cancelled().is_err());
    assert!(standalone_recovery.check_cancelled().is_err());
}

#[test]
fn cancelled_recovery_opens_no_native_resources() {
    let stopping = AtomicBool::new(true);
    assert!(
        DeviceStreams::open_inventory(
            &StreamOwners::default(),
            &DeviceRequirements::default(),
            3,
            &[],
            OpenPolicy::Recovery(&stopping),
            render_format()
        )
        .is_err()
    );
}
#[test]
fn no_device_graph_does_not_need_a_pipewire_server() {
    let mut streams = DeviceStreams::open(&DeviceRequirements::default(), 1).unwrap();
    let mut input = [1.0; 4];
    assert_eq!(streams.read("missing", &mut input).frames, 0);
    assert_eq!(input.map(f32::to_bits), [0; 4]);
    assert_eq!(streams.write("missing", &input).frames, 0);
    assert!(
        !streams
            .watch(DeviceRequirements::default())
            .automatic_outputs
    );
    let standalone = DeviceStreams::open_standalone_format(
        &StreamOwners::default(),
        &DeviceRequirements::default(),
        2,
        render_format(),
        None,
    )
    .unwrap();
    assert!(
        standalone
            .watch(DeviceRequirements::default())
            .automatic_outputs
    );
}
#[test]
fn gain_edits_do_not_reopen_native_devices() {
    let mut doc: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":1,"id":"test","revision":1,"nodes":[
            {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":64,"gainDb":0.0}}
        ],"edges":[]
    })).unwrap();
    let before = DeviceRequirements::from_document(&doc);
    if let NodeConfig::AudioDeviceInput(config) = &mut doc.nodes[0].config {
        config.gain_db = -12.0;
    }
    assert!(before == DeviceRequirements::from_document(&doc));
    if let NodeConfig::AudioDeviceInput(config) = &mut doc.nodes[0].config {
        config.output_channels = 16;
    }
    assert!(before != DeviceRequirements::from_document(&doc));
}
#[test]
fn health_watch_retries_only_disconnected_registered_streams() {
    let input = Arc::new(AtomicBool::new(true));
    let output = Arc::new(AtomicBool::new(true));
    let spec = DeviceRequirements {
        inputs: vec![(
            "mic".into(),
            AudioDeviceInputConfig {
                selector: DeviceSelector::DefaultInput,
                output_channels: 64,
                gain_db: 0.0,
            },
        )],
        outputs: vec![(
            "speaker".into(),
            AudioDeviceOutputConfig {
                selector: DeviceSelector::DefaultOutput,
                input_channels: 16,
            },
        )],
    };
    let watch = DeviceWatch {
        owners: StreamOwners::default(),
        automatic_outputs: false,
        format: render_format(),
        generation: 3,
        spec,
        inputs: BTreeMap::from([("mic".into(), input.clone())]),
        outputs: BTreeMap::from([("speaker".into(), output.clone())]),
        input_selections: BTreeMap::new(),
        output_selections: BTreeMap::new(),
    };
    assert!(watch.disconnected().is_empty());
    input.store(false, Ordering::Release);
    assert_eq!(watch.disconnected().inputs.len(), 1);
    assert!(watch.disconnected().outputs.is_empty());
    input.store(true, Ordering::Release);
    output.store(false, Ordering::Release);
    assert!(watch.disconnected().inputs.is_empty());
    assert_eq!(watch.disconnected().outputs.len(), 1);
}

#[test]
fn off_bindings_are_neither_retried_nor_kept_under_observation() {
    let connected = Arc::new(AtomicBool::new(false));
    let selection = Arc::new(DeviceSelection::new("mic", 10));
    let watch = DeviceWatch {
        owners: StreamOwners::default(),
        automatic_outputs: false,
        format: render_format(),
        generation: 3,
        spec: DeviceRequirements {
            inputs: vec![(
                "mic".into(),
                AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: 2,
                    gain_db: 0.0,
                },
            )],
            outputs: Vec::new(),
        },
        inputs: BTreeMap::from([("mic".into(), connected.clone())]),
        outputs: BTreeMap::new(),
        input_selections: BTreeMap::from([("mic".into(), selection.clone())]),
        output_selections: BTreeMap::new(),
    };
    assert!(watch.disconnected().is_empty());
    assert!(!watch.needs_observation());
    selection.enabled.store(true, Ordering::Release);
    assert_eq!(watch.disconnected().inputs.len(), 1);
    assert!(watch.needs_observation());
    connected.store(true, Ordering::Release);
    assert!(watch.disconnected().is_empty());
    assert!(
        watch.needs_observation(),
        "an enabled default binding keeps following the system default"
    );
}
