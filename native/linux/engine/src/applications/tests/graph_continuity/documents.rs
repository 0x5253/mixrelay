//! Graph documents the scenarios apply, and the composed catalog they rely on.
use super::*;

pub(super) fn application_capture_request(process_id: u32) -> ApplicationCaptureRequest {
    ApplicationCaptureRequest {
        selector: ApplicationSelector::ProcessId {
            process_id,
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
    }
}

pub(super) fn graph_document(process_id: u32, sink_node_name: &str) -> GraphDocument {
    let capture = application_capture_request(process_id);
    GraphDocument::new(
        1,
        vec![
            NodeSpec {
                id: "application".into(),
                label: Some("owned graph continuity source".into()),
                control: mixrelay_audio_graph::NodeControl::default(),
                config: NodeConfig::ApplicationCapture(capture),
            },
            NodeSpec {
                id: "engine-output".into(),
                label: Some("owned graph continuity sink".into()),
                control: mixrelay_audio_graph::NodeControl::default(),
                config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: sink_node_name.into(),
                    },
                    input_channels: 2,
                }),
            },
        ],
        vec![EdgeSpec {
            id: "application-to-sink".into(),
            source: AudioEndpoint {
                node_id: "application".into(),
                port_id: AUDIO_PORT_ID.into(),
            },
            destination: AudioEndpoint {
                node_id: "engine-output".into(),
                port_id: AUDIO_PORT_ID.into(),
            },
            channel_mapping: ChannelMapping::default(),
        }],
    )
}

pub(super) struct NativeApplicationResolver;

impl ApplicationIdentityResolver for NativeApplicationResolver {
    fn resolve(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<SharedSource, CompositionError> {
        crate::applications::identity::resolve(request)
            .map_err(|error| CompositionError::Invalid(error.to_string()))
    }
}

pub(super) fn forward_endpoint() -> VirtualEndpointSpec {
    VirtualEndpointSpec {
        id: FORWARD_ENDPOINT_ID.into(),
        display_name: "MixRelay private source-mute target".into(),
        render: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        capture: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        supported_sample_rates: vec![SAMPLE_RATE],
        default_sample_rate: SAMPLE_RATE,
    }
}

pub(super) fn source_mute_graph(process_id: u32, sink_node_name: &str) -> GraphDocument {
    let mut document = graph_document(process_id, sink_node_name);
    let NodeConfig::ApplicationCapture(request) = &mut document.nodes[0].config else {
        unreachable!("graph helper always creates application capture")
    };
    request.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
    request.forwarding_endpoint = Some(FORWARD_ENDPOINT_ID.into());
    document.nodes.push(NodeSpec {
        id: "owned-forward-endpoint".into(),
        label: Some("explicit private forwarding endpoint".into()),
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::VirtualEndpoint(VirtualEndpointConfig {
            spec: forward_endpoint(),
            reference: VirtualEndpointReference {
                render: true,
                capture: true,
            },
            gain_db: 0.0,
        }),
    });
    document
}

pub(super) fn composed_source_mute_graph(
    source_pid: u32,
    sink_node_name: &str,
) -> mixrelay_audio_graph::composition::ComposedGraph {
    let document = source_mute_graph(source_pid, sink_node_name);
    let devices = [AudioDeviceDescriptor {
        uid: sink_node_name.into(),
        display_name: "private graph observer".into(),
        input_channels: 0,
        output_channels: 2,
        is_default_input: false,
        is_default_output: false,
    }];
    compose_catalog(
        &[("source-mute".into(), document)],
        CatalogExecutionContext {
            revision: 1,
            compile: CompileContext {
                format: RenderFormat {
                    sample_rate: SAMPLE_RATE,
                    block_frames: BLOCK_FRAMES,
                },
                limits: CompileLimits::default(),
            },
            compiler: &GraphCompiler::default(),
            devices: &devices,
            applications: &NativeApplicationResolver,
        },
    )
    .expect("compose authored Forward intent with one canonical capture")
}

pub(super) fn composed_output_node_name(
    outputs: &BTreeMap<String, NodeId>,
    sink_node_name: &str,
) -> String {
    assert_eq!(
        outputs.len(),
        1,
        "composed graph must have one private sink output"
    );
    let native_output_key = format!("device:{}:{sink_node_name}", sink_node_name.len());
    let output_id = outputs
        .get(&native_output_key)
        .expect("composed graph retains the selected private sink output");
    format!(
        "{}.device-output",
        crate::runtime::sanitize_node_name(output_id.as_str())
    )
}

#[test]
pub(super) fn graph_document_is_exact_two_channel_capture_to_selected_sink() {
    let document = graph_document(1234, SINK_NODE_NAME);
    assert_eq!(document.nodes.len(), 2);
    assert_eq!(document.edges.len(), 1);
    assert_eq!(document.nodes[0].id.as_str(), "application");
    assert_eq!(document.nodes[1].id.as_str(), "engine-output");
    let NodeConfig::ApplicationCapture(capture) = &document.nodes[0].config else {
        panic!("first node must be the selected application capture");
    };
    assert_eq!(
        capture.selector,
        ApplicationSelector::ProcessId {
            process_id: 1234,
            include_descendants: false,
        }
    );
    assert_eq!(capture.source_playback, SourcePlaybackPolicy::Preserve);
    let NodeConfig::AudioDeviceOutput(output) = &document.nodes[1].config else {
        panic!("second node must be the selected AudioDeviceOutput");
    };
    assert!(matches!(
        &output.selector,
        DeviceSelector::PersistentUid { uid } if uid == SINK_NODE_NAME
    ));
    assert_eq!(output.input_channels, 2);
    assert_eq!(document.edges[0].source.node_id.as_str(), "application");
    assert_eq!(
        document.edges[0].destination.node_id.as_str(),
        "engine-output"
    );
    assert_eq!(
        ENGINE_OUTPUT_NODE_NAME,
        "io.mixrelay.engine-output.device-output"
    );
}

#[test]
pub(super) fn composed_output_name_uses_the_canonical_native_output_id() {
    let composed = composed_source_mute_graph(1234, SINK_NODE_NAME);
    assert_eq!(composed.native_outputs.len(), 1);
    let native_output_key = format!("device:{}:{SINK_NODE_NAME}", SINK_NODE_NAME.len());
    assert_eq!(
        composed.native_outputs[&native_output_key].as_str(),
        "o:39:device:29:io.mixrelay.fixture.mute-sink:native"
    );
    assert_eq!(
        composed_output_node_name(&composed.native_outputs, SINK_NODE_NAME),
        "io.mixrelay.o_39_device_29_io.mixrelay.fixture.mute-sink_native.device-output"
    );
}

#[test]
pub(super) fn composed_forward_intent_retains_exact_endpoint_without_authored_loop() {
    let authored = source_mute_graph(1234, SINK_NODE_NAME);
    assert_eq!(authored.edges.len(), 1);
    assert_eq!(authored.edges[0].id.as_str(), "application-to-sink");

    let composed = composed_source_mute_graph(1234, SINK_NODE_NAME);
    let endpoint_id = VirtualEndpointId::from(FORWARD_ENDPOINT_ID);
    assert!(composed.native_devices.contains_key(&endpoint_id));
    assert_eq!(composed.playback_intents.len(), 1);
    let intent = &composed.playback_intents[0];
    assert_eq!(intent.forwarding_endpoint, endpoint_id);
    assert!(
        composed
            .native_sources
            .values()
            .any(|id| id == &intent.capture_node_id)
    );
    assert!(
        composed
            .node_origins
            .get(&intent.mask_node_id)
            .is_some_and(|origin| {
                origin.graph_id == "source-mute" && origin.node_id.as_str() == "application"
            })
    );
}

#[test]
pub(super) fn composed_forward_endpoint_uses_canonical_playback_without_fixture_feedback() {
    let composed = composed_source_mute_graph(1234, SINK_NODE_NAME);
    let endpoint_id = VirtualEndpointId::from(FORWARD_ENDPOINT_ID);
    let native = &composed.native_devices[&endpoint_id];
    let snapshot = &composed.device_buses[&endpoint_id];
    let render_edges = composed
        .document
        .edges
        .iter()
        .filter(|edge| {
            edge.source.node_id == *native && edge.source.port_id.as_str() == FROM_OS_PORT_ID
        })
        .collect::<Vec<_>>();
    assert_eq!(render_edges.len(), 1);
    // Native capture already contains the Driver's canonical sum. Reading it
    // feeds the snapshot directly; feeding it back to the render mixer would
    // duplicate that signal and create an unauthored feedback path.
    assert_eq!(&render_edges[0].destination.node_id, snapshot);
    assert_eq!(render_edges[0].destination.port_id.as_str(), AUDIO_PORT_ID);
    assert!(
        !composed.document.edges.iter().any(|edge| {
            edge.source.node_id == *snapshot && edge.destination.node_id == *native
        })
    );
    assert!(!composed.document.edges.iter().any(|edge| {
        let source = composed.node_origins.get(&edge.source.node_id);
        let destination = composed.node_origins.get(&edge.destination.node_id);
        source.is_some_and(|origin| {
            origin.node_id.as_str() == "owned-forward-endpoint"
                && origin
                    .meter_port
                    .as_ref()
                    .is_some_and(|port| port.as_str() == FROM_OS_PORT_ID)
        }) && destination.is_some_and(|origin| {
            origin.node_id.as_str() == "owned-forward-endpoint"
                && origin
                    .meter_port
                    .as_ref()
                    .is_some_and(|port| port.as_str() == TO_OS_PORT_ID)
        })
    }));
}
