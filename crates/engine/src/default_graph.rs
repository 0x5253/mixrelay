//! Built-in session documents and their explicit channel routes.

use crate::{
    AUDIO_PORT_ID, AudioEndpoint, ChannelLayout, ChannelMapping, DEFAULT_ENDPOINT_NODE_ID, EdgeId,
    EdgeSpec, FROM_OS_PORT_ID, GainConfig, GraphDocument, NodeConfig, NodeId, NodeSpec, PortId,
    RenderFormat, StreamSpec, TO_OS_PORT_ID, VirtualEndpointId, VirtualEndpointSpec,
};

#[must_use]
pub fn default_session_document(revision: u64, format: RenderFormat) -> GraphDocument {
    let mut document = default_duplex_document(revision, None);
    for node in &mut document.nodes {
        if let NodeConfig::VirtualEndpoint(endpoint) = &mut node.config {
            endpoint.default_sample_rate = format.sample_rate;
            endpoint.supported_sample_rates = vec![format.sample_rate];
        }
    }
    document
}

#[must_use]
pub fn default_duplex_document(revision: u64, gain_db: Option<f32>) -> GraphDocument {
    let endpoint_id = NodeId::from(DEFAULT_ENDPOINT_NODE_ID);
    let endpoint = NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: endpoint_id.clone(),
        label: Some("MixRelay Virtual I/O".into()),
        config: NodeConfig::VirtualEndpoint(
            VirtualEndpointSpec {
                id: VirtualEndpointId::from(DEFAULT_ENDPOINT_NODE_ID),
                display_name: "MixRelay Virtual I/O".into(),
                render: Some(stereo_stream()),
                capture: Some(stereo_stream()),
                supported_sample_rates: vec![48_000],
                default_sample_rate: 48_000,
            }
            .into(),
        ),
    };
    match gain_db {
        None => GraphDocument::new(
            revision,
            vec![endpoint],
            (0..2)
                .map(|channel| {
                    channel_edge(
                        &format!("default-pass-through-{}", channel + 1),
                        channel_endpoint(DEFAULT_ENDPOINT_NODE_ID, FROM_OS_PORT_ID, channel),
                        channel_endpoint(DEFAULT_ENDPOINT_NODE_ID, TO_OS_PORT_ID, channel),
                    )
                })
                .collect(),
        ),
        Some(gain_db) => GraphDocument::new(
            revision,
            vec![
                endpoint,
                NodeSpec {
                    control: mixrelay_audio_graph::NodeControl::default(),
                    id: NodeId::from("output-gain"),
                    label: Some("Output gain".into()),
                    config: NodeConfig::Gain(GainConfig {
                        channels: 2,
                        gain_db,
                    }),
                },
            ],
            (0..2)
                .flat_map(|channel| {
                    [
                        channel_edge(
                            &format!("from-os-to-gain-{}", channel + 1),
                            channel_endpoint(DEFAULT_ENDPOINT_NODE_ID, FROM_OS_PORT_ID, channel),
                            channel_endpoint("output-gain", AUDIO_PORT_ID, channel),
                        ),
                        channel_edge(
                            &format!("gain-to-os-{}", channel + 1),
                            channel_endpoint("output-gain", AUDIO_PORT_ID, channel),
                            channel_endpoint(DEFAULT_ENDPOINT_NODE_ID, TO_OS_PORT_ID, channel),
                        ),
                    ]
                })
                .collect(),
        ),
    }
}

const fn stereo_stream() -> StreamSpec {
    StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    }
}

fn audio_endpoint(node_id: &str, port_id: &str) -> AudioEndpoint {
    AudioEndpoint {
        node_id: NodeId::from(node_id),
        port_id: PortId::from(port_id),
    }
}

#[derive(Debug, Clone, Copy)]
struct ChannelEndpoint<'a> {
    node_id: &'a str,
    port_id: &'a str,
    channel: u16,
}

const fn channel_endpoint<'a>(
    node_id: &'a str,
    port_id: &'a str,
    channel: u16,
) -> ChannelEndpoint<'a> {
    ChannelEndpoint {
        node_id,
        port_id,
        channel,
    }
}

fn channel_edge(
    id: &str,
    source: ChannelEndpoint<'_>,
    destination: ChannelEndpoint<'_>,
) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: audio_endpoint(source.node_id, source.port_id),
        destination: audio_endpoint(destination.node_id, destination.port_id),
        channel_mapping: ChannelMapping::Explicit {
            routes: vec![mixrelay_audio_graph::ChannelRoute {
                source_channel: source.channel,
                destination_channel: destination.channel,
            }],
        },
    }
}
