//! Built-in node configuration and port-resolution contracts.
use super::*;

#[test]
fn recorder_resolves_one_to_eight_input_channels_without_native_output_ports() {
    for channels in 1..=mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS {
        let node = configured_node("recorder", NodeConfig::Recorder { channels });
        let resolved = BuiltInNodeResolver.resolve(&node, &context()).unwrap();
        assert!(resolved.outputs.is_empty());
        assert_eq!(resolved.inputs.len(), 1);
        assert_eq!(resolved.inputs[0].channels(), channels);
    }
    for channels in [0, 9, 64] {
        let node = configured_node("recorder", NodeConfig::Recorder { channels });
        assert!(BuiltInNodeResolver.resolve(&node, &context()).is_err());
    }
}

#[test]
fn built_in_resolver_supports_dynamic_selectors_and_all_node_kinds() {
    let configurations = [
        app(ApplicationSelector::ExecutablePath {
            path: "/Applications/Firefox.app/Contents/MacOS/firefox".into(),
        }),
        app(ApplicationSelector::AllExcept {
            bundle_ids: vec!["com.apple.Music".into()],
        }),
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::PersistentUid {
                uid: "mic-uid".into(),
            },
            output_channels: 1,
            gain_db: 0.0,
        }),
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultInput,
            input_channels: 2,
        }),
        NodeConfig::Gain(GainConfig {
            channels: 2,
            gain_db: -6.0,
        }),
    ];

    for (index, config) in configurations.into_iter().enumerate() {
        let resolved = BuiltInNodeResolver
            .resolve(
                &configured_node(&format!("node-{index}"), config),
                &context(),
            )
            .expect("configuration should resolve");
        assert!((1..=2).contains(&(resolved.inputs.len() + resolved.outputs.len())));
    }
}

#[test]
fn built_in_resolver_rejects_every_invalid_configuration_class() {
    let invalid = [
        app(ApplicationSelector::BundleId {
            bundle_id: " ".into(),
            include_descendants: true,
        }),
        app_with_channels(ApplicationSelector::AllExcept { bundle_ids: vec![] }, 0),
        app(ApplicationSelector::ExecutablePath {
            path: String::new(),
        }),
        app(ApplicationSelector::AllExcept {
            bundle_ids: vec![String::new()],
        }),
        device_input(DeviceSelector::PersistentUid { uid: String::new() }, 2),
        device_input(DeviceSelector::DefaultInput, 0),
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: 0,
        }),
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::PersistentUid { uid: String::new() },
            input_channels: 2,
        }),
        mixer(vec![], 2),
        mixer(vec![PortId::from("input")], 0),
        mixer(vec![PortId::from("")], 2),
        mixer(vec![PortId::from("one"), PortId::from("one")], 2),
        gain(0, 0.0),
        gain(2, f32::INFINITY),
        virtual_endpoint("", "MixRelay", None, Some(2)),
        virtual_endpoint("uid", "", None, Some(2)),
        virtual_endpoint("uid", "MixRelay", None, Some(65)),
    ];

    for (index, config) in invalid.into_iter().enumerate() {
        let error = BuiltInNodeResolver
            .resolve(
                &configured_node(&format!("invalid-{index}"), config),
                &context(),
            )
            .expect_err("configuration should be rejected");
        assert!(matches!(
            error,
            NodeResolutionError::InvalidConfiguration { .. }
        ));
    }
}
