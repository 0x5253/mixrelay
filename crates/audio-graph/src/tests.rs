//! Shared graph fixtures for document, resolver and compilation contract tests.
use crate::{
    AUDIO_PORT_ID, ApplicationCaptureRequest, ApplicationSelector, AudioDeviceInputConfig,
    AudioDeviceOutputConfig, AudioEndpoint, BuiltInNodeResolver, ChannelLayout, ChannelMapping,
    ChannelRoute, CompileContext, CompileError, CompileLimits, CompiledPort, DeviceSelector,
    EdgeId, EdgeSpec, FROM_OS_PORT_ID, GainConfig, GraphCompiler, GraphDocument, GraphIssue,
    GraphIssueCode, GraphResource, MixerConfig, NodeConfig, NodeId, NodeKind, NodeResolutionError,
    NodeResolver, NodeSpec, NodeTypeId, PortDirection, PortId, ProviderNodeConfig, RenderFormat,
    ResolvedNode, SourcePlaybackPolicy, StreamSpec, TO_OS_PORT_ID, VirtualEndpointId,
    VirtualEndpointReference, VirtualEndpointSpec, validate_virtual_endpoint_gain,
};
use mixrelay_audio_io::{
    ApplicationCaptureCapabilities, ApplicationSelectorKind, VirtualEndpointCapabilities,
};
use std::collections::BTreeSet;

mod builtin_nodes;
mod compilation;
mod diagnostics;
mod documents;
mod dormant_provider_tests;
mod provider_resolution;

fn context() -> CompileContext {
    CompileContext {
        format: RenderFormat {
            sample_rate: 96_000,
            block_frames: 256,
        },
        limits: CompileLimits::default(),
    }
}

fn endpoint(node_id: &str) -> AudioEndpoint {
    AudioEndpoint {
        node_id: NodeId::from(node_id),
        port_id: PortId::from(AUDIO_PORT_ID),
    }
}

fn gain_node(id: &str) -> NodeSpec {
    NodeSpec {
        id: NodeId::from(id),
        label: None,
        control: crate::NodeControl::default(),
        config: NodeConfig::Gain(GainConfig {
            channels: 2,
            gain_db: 0.0,
        }),
    }
}

fn configured_node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: NodeId::from(id),
        label: None,
        control: crate::NodeControl::default(),
        config,
    }
}

fn edge(id: &str, source: &str, destination: &str) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: endpoint(source),
        destination: endpoint(destination),
        channel_mapping: ChannelMapping::Auto,
    }
}

fn compile_error(document: &GraphDocument) -> CompileError {
    GraphCompiler::default()
        .compile(document, context())
        .expect_err("graph should be rejected")
}

fn issue_codes(error: &CompileError) -> Vec<GraphIssueCode> {
    error.issues().iter().map(GraphIssue::code).collect()
}

fn app(selector: ApplicationSelector) -> NodeConfig {
    app_with_channels(selector, 2)
}

fn app_with_channels(selector: ApplicationSelector, channels: u16) -> NodeConfig {
    NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
        selector,
        stream: StreamSpec {
            channels,
            channel_layout: if channels == 1 {
                ChannelLayout::Mono
            } else {
                ChannelLayout::Discrete
            },
        },
        source_playback: SourcePlaybackPolicy::Preserve,
        forwarding_endpoint: None,
        gain_db: 0.0,
        requires_reselection: false,
    })
}

fn device_input(selector: DeviceSelector, output_channels: u16) -> NodeConfig {
    NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
        selector,
        output_channels,
        gain_db: 0.0,
    })
}

fn mixer(inputs: Vec<PortId>, channels: u16) -> NodeConfig {
    NodeConfig::Mixer(MixerConfig { inputs, channels })
}

fn gain(channels: u16, gain_db: f32) -> NodeConfig {
    NodeConfig::Gain(GainConfig { channels, gain_db })
}

fn virtual_endpoint(
    id: &str,
    display_name: &str,
    render_channels: Option<u16>,
    capture_channels: Option<u16>,
) -> NodeConfig {
    NodeConfig::VirtualEndpoint(
        VirtualEndpointSpec {
            id: VirtualEndpointId::from(id),
            display_name: display_name.into(),
            render: render_channels.map(discrete_stream),
            capture: capture_channels.map(discrete_stream),
            supported_sample_rates: vec![48_000, 96_000],
            default_sample_rate: 48_000,
        }
        .into(),
    )
}

const fn discrete_stream(channels: u16) -> StreamSpec {
    StreamSpec {
        channels,
        channel_layout: ChannelLayout::Discrete,
    }
}

fn provider(type_id: &str) -> NodeConfig {
    NodeConfig::Provider(ProviderNodeConfig {
        ports: None,
        type_id: NodeTypeId::from(type_id),
        parameters: serde_json::json!({ "thresholdDb": -12.0 }),
    })
}
