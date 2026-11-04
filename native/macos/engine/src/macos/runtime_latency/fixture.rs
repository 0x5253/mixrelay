//! Feedback and snapshot-route plans shared by latency tests in this crate.
use crate::macos_devices::latency::Observation;
use mixrelay_render::RenderPlan;
use std::sync::Arc;

pub(crate) fn feedback_plan() -> RenderPlan {
    RenderPlan::prepare(&feedback_execution()).unwrap()
}
pub(crate) fn feedback_processor(
    factory: &dyn mixrelay_render::HostedProcessorFactory,
    meters: Arc<mixrelay_render::PeakMeterBank>,
) -> mixrelay_render::BlockProcessor {
    mixrelay_render::BlockProcessor::prepare_with_hosted_processors(
        &feedback_execution(),
        meters,
        factory,
    )
    .unwrap()
}
fn feedback_execution() -> mixrelay_audio_graph::ExecutionPlan {
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
    };
    let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[{
            "id":"virtual","type":"virtual_endpoint",
            "config":{"id":"01010101-0101-0101-0101-010101010101","displayName":"Fixture",
                "reference":{"render":true,"capture":true},
                "render":{"channels":2,"channelLayout":"stereo"},"capture":{"channels":2,"channelLayout":"stereo"},
                "supportedSampleRates":[48000],"defaultSampleRate":48000}
        }, {"id":"gain","type":"gain","config":{"channels":2,"gainDb":0}}],"edges":[
            {"id":"input","source":{"nodeId":"virtual","portId":"from_os"},"destination":{"nodeId":"gain","portId":"audio"}},
            {"id":"feedback","source":{"nodeId":"gain","portId":"audio"},"destination":{"nodeId":"virtual","portId":"to_os"}}]
    })).unwrap();
    if let mixrelay_audio_graph::NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config {
        document.nodes[0].config =
            mixrelay_audio_graph::NodeConfig::VirtualEndpointTransport(endpoint.spec.clone());
    }
    GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap()
}
pub(crate) fn observation() -> Observation {
    use mixrelay_audio_io::residence::ResidenceSnapshot;
    use mixrelay_macos_transport::device_io::LatencySnapshot;
    Observation {
        identity: 1,
        epoch: 2,
        graph_epoch: 5,
        generation: 3,
        clock_breaks: 0,
        native_rate: 48_000,
        graph_rate: 48_000,
        direct: true,
        ready: true,
        sampled: true,
        faults: 0,
        native: LatencySnapshot::default(),
        src_input: ResidenceSnapshot::default(),
        src_output: ResidenceSnapshot::default(),
        src_delay: 0,
    }
}

pub(crate) fn canonical_snapshot_route() -> mixrelay_audio_graph::composition::ComposedGraph {
    use mixrelay_audio_graph::composition::{
        DeviceReadReference, DeviceReference, DeviceRegistry, GraphFragment, compose,
    };
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeConfig, RenderFormat,
    };
    let fixture = feedback_execution();
    let mut devices = Vec::new();
    for id in ["A", "D"] {
        let NodeConfig::VirtualEndpointTransport(spec) = fixture
            .nodes()
            .iter()
            .find_map(|node| {
                matches!(node.config(), NodeConfig::VirtualEndpointTransport(_))
                    .then_some(node.config())
            })
            .unwrap()
        else {
            unreachable!()
        };
        let mut spec = spec.clone();
        spec.id = id.into();
        devices.push(spec);
    }
    let registry = DeviceRegistry::new(devices).unwrap();
    let document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"gain","type":"gain","config":{"channels":2,"gainDb":0}}
        ],"edges":[
            {"id":"in","source":{"nodeId":"a","portId":"from_os"},"destination":{"nodeId":"gain","portId":"audio"}},
            {"id":"out","source":{"nodeId":"gain","portId":"audio"},"destination":{"nodeId":"d","portId":"to_os"}}
        ]
    })).unwrap();
    let references = [
        DeviceReference {
            node_id: "a".into(),
            device_id: "A".into(),
            control: mixrelay_audio_graph::NodeControl::default(),
            read: Some(DeviceReadReference { channels: 2 }),
            write_channels: None,
            gain_db: 0.0,
        },
        DeviceReference {
            node_id: "d".into(),
            device_id: "D".into(),
            control: mixrelay_audio_graph::NodeControl::default(),
            read: None,
            write_channels: Some(2),
            gain_db: 0.0,
        },
    ];
    compose(
        &registry,
        &[GraphFragment {
            id: "route",
            document: &document,
            device_references: &references,
            source_references: &[],
            output_references: &[],
        }],
        1,
        CompileContext {
            format: RenderFormat {
                sample_rate: 48_000,
                block_frames: 32,
            },
            limits: CompileLimits::default(),
        },
        &GraphCompiler::default(),
    )
    .unwrap()
}
