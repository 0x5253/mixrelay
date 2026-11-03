use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping,
    EdgeId, EdgeSpec, GraphDocument, MixerConfig, NodeConfig, NodeControl, NodeId, NodeSpec,
    PortId, ProviderNodeConfig,
};

pub(super) fn document(type_id: &str, parameters: serde_json::Value) -> GraphDocument {
    let nodes = vec![
        node(
            "in",
            NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: mixrelay_audio_graph::DeviceSelector::DefaultInput,
                output_channels: 2,
                gain_db: 0.0,
            }),
        ),
        node(
            "effect",
            NodeConfig::Provider(ProviderNodeConfig {
                type_id: type_id.into(),
                ports: None,
                parameters,
            }),
        ),
        node(
            "mix",
            NodeConfig::Mixer(MixerConfig {
                inputs: vec![PortId::from("dry"), PortId::from("wet")],
                channels: 2,
            }),
        ),
        node(
            "out",
            NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: mixrelay_audio_graph::DeviceSelector::DefaultOutput,
                input_channels: 2,
            }),
        ),
    ];
    let edges = vec![
        edge(
            "input-effect",
            ("in", AUDIO_PORT_ID),
            ("effect", AUDIO_PORT_ID),
        ),
        edge("input-mix", ("in", AUDIO_PORT_ID), ("mix", "dry")),
        edge("effect-mix", ("effect", AUDIO_PORT_ID), ("mix", "wet")),
        edge("mix-output", ("mix", AUDIO_PORT_ID), ("out", AUDIO_PORT_ID)),
    ];
    GraphDocument::new(1, nodes, edges)
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: NodeId::from(id),
        label: None,
        control: NodeControl::default(),
        config,
    }
}

fn edge(id: &str, source: (&str, &str), destination: (&str, &str)) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: endpoint(source),
        destination: endpoint(destination),
        channel_mapping: ChannelMapping::Auto,
    }
}

fn endpoint((node_id, port_id): (&str, &str)) -> AudioEndpoint {
    AudioEndpoint {
        node_id: node_id.into(),
        port_id: port_id.into(),
    }
}
