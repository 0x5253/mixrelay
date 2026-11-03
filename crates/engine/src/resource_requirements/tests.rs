use super::*;
use mixrelay_audio_graph::{DeviceSelector, NodeSpec};
use mixrelay_audio_io::{ApplicationSelector, ChannelLayout, SourcePlaybackPolicy, StreamSpec};

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: id.into(),
        label: None,
        config,
    }
}

fn document() -> GraphDocument {
    GraphDocument::new(
        7,
        vec![
            node(
                "shared:mic",
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: "hardware-mic".into(),
                    },
                    output_channels: 8,
                    gain_db: -12.0,
                }),
            ),
            node(
                "shared:speaker",
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::DefaultOutput,
                    input_channels: 2,
                }),
            ),
            node(
                "shared:app",
                NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
                    selector: ApplicationSelector::BundleId {
                        bundle_id: "io.test.player".into(),
                        include_descendants: false,
                    },
                    stream: StreamSpec {
                        channels: 2,
                        channel_layout: ChannelLayout::Stereo,
                    },
                    source_playback: SourcePlaybackPolicy::Preserve,
                    forwarding_endpoint: None,
                    gain_db: -6.0,
                    requires_reselection: false,
                }),
            ),
            node("recorder", NodeConfig::Recorder { channels: 8 }),
        ],
        vec![],
    )
}

#[test]
fn gain_label_and_revision_edits_do_not_change_native_identity_or_mutate_the_graph() {
    let original = document();
    let expected = ResourceRequirements::from_document(&original);
    let mut edited = original.clone();
    edited.revision += 1;
    for node in &mut edited.nodes {
        node.label = Some("renamed graph view".into());
        match &mut node.config {
            NodeConfig::AudioDeviceInput(config) => config.gain_db = 3.0,
            NodeConfig::ApplicationCapture(config) => config.gain_db = -24.0,
            _ => {}
        }
    }
    assert_eq!(expected, ResourceRequirements::from_document(&edited));
    assert_eq!(original, document());
    assert_eq!(
        expected.devices.inputs[0].1.gain_db.to_bits(),
        0.0_f32.to_bits()
    );
    assert_eq!(expected.captures[0].1.gain_db.to_bits(), 0.0_f32.to_bits());
}

#[test]
fn binding_ids_widths_and_capture_policy_survive_extraction() {
    let original = document();
    let expected = ResourceRequirements::from_document(&original);
    assert_eq!(
        expected.devices,
        DeviceRequirements::from_document(&original)
    );
    assert_eq!(expected.devices.inputs[0].0, "shared:mic");
    assert_eq!(expected.devices.inputs[0].1.output_channels, 8);
    assert_eq!(expected.devices.outputs[0].0, "shared:speaker");
    assert_eq!(expected.captures[0].0, "shared:app");
    assert_eq!(
        expected.captures[0].1.source_playback,
        SourcePlaybackPolicy::Preserve
    );
    let mut edited = original;
    let NodeConfig::AudioDeviceInput(input) = &mut edited.nodes[0].config else {
        panic!("input fixture")
    };
    input.output_channels = 4;
    assert_ne!(expected, ResourceRequirements::from_document(&edited));
}

#[test]
fn every_composed_endpoint_is_kept_without_inventing_transports_for_authored_references() {
    let mut graph = crate::default_duplex_document(1, None);
    let NodeConfig::VirtualEndpoint(config) = &graph.nodes[0].config else {
        panic!("endpoint fixture")
    };
    let first = config.spec.clone();
    let mut second = first.clone();
    second.id = "second-endpoint".into();
    graph.nodes.push(node(
        "transport:first",
        NodeConfig::VirtualEndpointTransport(first),
    ));
    graph.nodes.push(node(
        "transport:second",
        NodeConfig::VirtualEndpointTransport(second.clone()),
    ));
    let requirements = ResourceRequirements::from_document(&graph);
    assert_eq!(requirements.endpoints.len(), 2);
    assert_eq!(requirements.endpoints[0].0, "transport:first");
    assert_eq!(
        requirements.endpoints[1],
        ("transport:second".into(), second)
    );
}

#[test]
fn removing_all_sources_requires_no_native_devices_or_captures() {
    let graph = GraphDocument::new(
        8,
        vec![node("recorder", NodeConfig::Recorder { channels: 2 })],
        vec![],
    );
    let requirements = ResourceRequirements::from_document(&graph);
    assert!(requirements.devices.is_empty());
    assert!(requirements.captures.is_empty());
    assert!(requirements.endpoints.is_empty());
}
