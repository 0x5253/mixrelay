use super::HEAP_OPERATIONS;
use crate::{BlockProcessor, RenderPlan};
use mixrelay_audio_graph::composition::*;
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioEndpoint, CompileContext, EdgeId, EdgeSpec, FROM_OS_PORT_ID, GainConfig,
    GraphCompiler, GraphDocument, NodeConfig, NodeId, NodeSpec, PortId, TO_OS_PORT_ID,
    VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, ChannelLayout, CompileLimits, DeviceSelector, RenderFormat, StreamSpec,
};
use std::collections::BTreeMap;
use std::sync::Arc;

mod authored_tests;
mod control_tests;
mod device_mix_tests;
mod lifetime_tests;
mod output_tests;

fn context() -> CompileContext {
    CompileContext {
        format: RenderFormat {
            sample_rate: 48_000,
            block_frames: 4,
        },
        limits: CompileLimits::default(),
    }
}
fn device(id: &str) -> VirtualEndpointSpec {
    VirtualEndpointSpec {
        id: VirtualEndpointId::from(id),
        display_name: format!("Device {id}"),
        render: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        capture: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        supported_sample_rates: vec![44_100, 48_000],
        default_sample_rate: 48_000,
    }
}
fn reference(node: &str, device: &str, read: bool, write: bool) -> DeviceReference {
    DeviceReference {
        control: mixrelay_audio_graph::NodeControl::default(),
        node_id: NodeId::from(node),
        device_id: VirtualEndpointId::from(device),
        read: read.then_some(DeviceReadReference { channels: 2 }),
        write_channels: write.then_some(2),
        gain_db: 0.0,
    }
}
fn edge(id: &str, source: &str, source_port: &str, sink: &str, sink_port: &str) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: endpoint(NodeId::from(source), source_port),
        destination: endpoint(NodeId::from(sink), sink_port),
        channel_mapping: mixrelay_audio_graph::ChannelMapping::default(),
    }
}
fn gain(node: &str, gain_db: f32) -> NodeSpec {
    NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from(node),
        label: None,
        config: NodeConfig::Gain(GainConfig {
            channels: 2,
            gain_db,
        }),
    }
}
fn source_document() -> GraphDocument {
    GraphDocument::new(
        1,
        vec![gain("gain", 0.0)],
        vec![
            edge("one", "mic", AUDIO_PORT_ID, "gain", AUDIO_PORT_ID),
            edge("two", "gain", AUDIO_PORT_ID, "shared", TO_OS_PORT_ID),
        ],
    )
}
fn mic_registry(devices: Vec<VirtualEndpointSpec>) -> DeviceRegistry {
    DeviceRegistry::new(devices)
        .unwrap()
        .with_sources(vec![SharedSource {
            id: "macbook-mic".into(),
            config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::PersistentUid {
                    uid: "macbook-mic".into(),
                },
                output_channels: 2,
                gain_db: 0.0,
            }),
        }])
        .unwrap()
}
fn mic_reference(gain_db: f32) -> SourceReference {
    SourceReference {
        control: mixrelay_audio_graph::NodeControl::default(),
        node_id: NodeId::from("mic"),
        source_id: "macbook-mic".into(),
        channels: 2,
        gain_db,
        forwarding_endpoint: None,
    }
}
fn bus_document(destination: &str, gain_db: f32) -> GraphDocument {
    GraphDocument::new(
        1,
        vec![gain("gain", gain_db)],
        vec![
            edge("one", "shared", FROM_OS_PORT_ID, "gain", AUDIO_PORT_ID),
            edge("two", "gain", AUDIO_PORT_ID, destination, TO_OS_PORT_ID),
        ],
    )
}

/// Executes the real composed render plan, not a graph metadata or topology simulation.
/// Driver X capture is [0.8, 0.6]; a shared mic/app supplies [0.2, 0.4].
/// The graph contribution sink must never re-send X's existing capture mix.
fn pcm(composed: &ComposedGraph) -> BTreeMap<NodeId, Vec<f32>> {
    let plan = Arc::new(RenderPlan::prepare(&composed.execution).unwrap());
    let sources: Vec<Vec<f32>> = plan
        .source_bindings()
        .iter()
        .map(|binding| {
            let frame = if composed
                .native_sources
                .values()
                .any(|id| id == binding.node_id())
            {
                [0.2, 0.4]
            } else if binding.node_id() == &device_node_id(&VirtualEndpointId::from("X"), "native")
            {
                [0.8, 0.6]
            } else {
                [0.0, 0.0]
            };
            frame.repeat(4)
        })
        .collect();
    let mut sinks: Vec<Vec<f32>> = plan
        .sink_bindings()
        .iter()
        .map(|binding| vec![0.0; usize::from(binding.channels()) * 4])
        .collect();
    let mut processor = BlockProcessor::new(plan.clone()).unwrap();
    for _ in 0..2 {
        processor
            .process_block(
                4,
                &sources.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                &mut sinks.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
            )
            .unwrap();
    }
    plan.sink_bindings()
        .iter()
        .map(|binding| binding.node_id().clone())
        .zip(sinks)
        .collect()
}
fn assert_frame(output: &[f32], frame: [f32; 2]) {
    for pair in output.chunks_exact(2) {
        assert!((pair[0] - frame[0]).abs() < 0.00001);
        assert!((pair[1] - frame[1]).abs() < 0.00001);
    }
}

#[test]
fn graph_contribution_excludes_driver_capture_and_reads_driver_mix_before_local_gain() {
    let registry = mic_registry(vec![device("X"), device("Y"), device("Z")]);
    let a = source_document();
    let b = bus_document("out", -6.020_6);
    let c = GraphDocument::new(
        1,
        vec![],
        vec![edge("one", "shared", FROM_OS_PORT_ID, "out", TO_OS_PORT_ID)],
    );
    let a_refs = [reference("shared", "X", false, true)];
    let b_refs = [
        reference("shared", "X", true, false),
        reference("out", "Y", false, true),
    ];
    let c_refs = [
        reference("shared", "X", true, false),
        reference("out", "Z", false, true),
    ];
    let composed = compose(
        &registry,
        &[
            GraphFragment {
                id: "A",
                document: &a,
                device_references: &a_refs,
                source_references: &[mic_reference(0.0)],
                output_references: &[],
            },
            GraphFragment {
                id: "B",
                document: &b,
                device_references: &b_refs,
                source_references: &[],
                output_references: &[],
            },
            GraphFragment {
                id: "C",
                document: &c,
                device_references: &c_refs,
                source_references: &[],
                output_references: &[],
            },
        ],
        50,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    let output = pcm(&composed);
    assert_frame(
        &output[&composed.native_devices[&VirtualEndpointId::from("X")]],
        [0.2, 0.4],
    );
    assert_frame(
        &output[&composed.native_devices[&VirtualEndpointId::from("Y")]],
        [0.4, 0.3],
    );
    assert_frame(
        &output[&composed.native_devices[&VirtualEndpointId::from("Z")]],
        [0.8, 0.6],
    );
    assert_ne!(
        runtime_node_id("A", &NodeId::from("gain")),
        runtime_node_id("B", &NodeId::from("gain"))
    );
    assert_eq!(
        composed.node_origins[&runtime_node_id("B", &NodeId::from("gain"))].graph_id,
        "B"
    );
    assert_eq!(composed.document.revision, 50);
}

#[test]
fn cross_graph_device_routes_execute_through_normal_io_snapshots() {
    let registry = DeviceRegistry::new(vec![device("X"), device("Y")]).unwrap();
    let doc = bus_document("out", 0.0);
    let a = [
        reference("shared", "X", true, false),
        reference("out", "Y", false, true),
    ];
    let b = [
        reference("shared", "Y", true, false),
        reference("out", "X", false, true),
    ];
    let composed = compose(
        &registry,
        &[
            GraphFragment {
                id: "A",
                document: &doc,
                device_references: &a,
                source_references: &[],
                output_references: &[],
            },
            GraphFragment {
                id: "B",
                document: &doc,
                device_references: &b,
                source_references: &[],
                output_references: &[],
            },
        ],
        2,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    let output = pcm(&composed);
    assert_frame(
        &output[&composed.native_devices[&VirtualEndpointId::from("X")]],
        [0.0, 0.0],
    );
    assert_frame(
        &output[&composed.native_devices[&VirtualEndpointId::from("Y")]],
        [0.8, 0.6],
    );
}

#[test]
fn only_referenced_named_devices_keep_independent_rates_and_last_removal_drops_native_inventory() {
    let mut x = device("X");
    x.default_sample_rate = 44_100;
    let registry = DeviceRegistry::new(vec![x.clone(), device("Y"), device("stale")]).unwrap();
    let document = GraphDocument::new(1, vec![], vec![]);
    let refs = [
        reference("x", "X", false, true),
        reference("y", "Y", false, true),
    ];
    let composed = compose(
        &registry,
        &[GraphFragment {
            id: "A",
            document: &document,
            device_references: &refs,
            source_references: &[],
            output_references: &[],
        }],
        3,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    assert_eq!(composed.native_devices.len(), 2);
    let NodeConfig::VirtualEndpointTransport(canonical) = &composed
        .document
        .nodes
        .iter()
        .find(|node| node.id == composed.native_devices[&x.id])
        .unwrap()
        .config
    else {
        panic!()
    };
    let mut client = x.clone();
    client.capture = None;
    assert_eq!(*canonical, client);
    assert!(
        !composed
            .native_devices
            .contains_key(&VirtualEndpointId::from("stale"))
    );
    assert_frame(&pcm(&composed)[&composed.native_devices[&x.id]], [0.0, 0.0]);
    assert_eq!(registry.get(&x.id), Some(&x));
    let removed = compose(&registry, &[], 4, context(), &GraphCompiler::default()).unwrap();
    assert!(removed.native_devices.is_empty());
    assert!(removed.device_buses.is_empty());
    assert!(removed.document.nodes.is_empty());
    assert!(removed.document.edges.is_empty());
    assert!(pcm(&removed).is_empty());
}

#[test]
fn multiple_graph_writers_sum_only_contributions_while_reader_uses_driver_capture() {
    let registry = mic_registry(vec![device("X"), device("Y")]);
    let a = source_document();
    let b = source_document();
    let c = bus_document("out", 0.0);
    let a_refs = [reference("shared", "X", false, true)];
    let b_refs = [reference("shared", "X", false, true)];
    let c_refs = [
        reference("shared", "X", true, false),
        reference("out", "Y", false, true),
    ];
    let composed = compose(
        &registry,
        &[
            GraphFragment {
                id: "A",
                document: &a,
                device_references: &a_refs,
                source_references: &[mic_reference(0.0)],
                output_references: &[],
            },
            GraphFragment {
                id: "B",
                document: &b,
                device_references: &b_refs,
                source_references: &[mic_reference(0.0)],
                output_references: &[],
            },
            GraphFragment {
                id: "C",
                document: &c,
                device_references: &c_refs,
                source_references: &[],
                output_references: &[],
            },
        ],
        2,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    let out = pcm(&composed);
    for (id, expected) in [("X", [0.4, 0.8]), ("Y", [0.8, 0.6])] {
        assert_frame(
            &out[&composed.native_devices[&VirtualEndpointId::from(id)]],
            expected,
        );
    }
}

#[test]
fn unknown_identity_wrong_tap_duplicate_nodes_and_budget_fail_closed() {
    let registry = DeviceRegistry::new(vec![device("X")]).unwrap();
    let compiler = GraphCompiler::default();
    let doc = GraphDocument::new(
        1,
        vec![],
        vec![edge("e", "ref", FROM_OS_PORT_ID, "ref", TO_OS_PORT_ID)],
    );
    for refs in [
        vec![reference("ref", "missing", true, true)],
        vec![reference("ref", "X", false, true)],
        vec![
            reference("ref", "X", true, true),
            reference("ref", "X", false, true),
        ],
    ] {
        assert!(
            compose(
                &registry,
                &[GraphFragment {
                    id: "g",
                    document: &doc,
                    device_references: &refs,
                    source_references: &[],
                    output_references: &[],
                }],
                1,
                context(),
                &compiler
            )
            .is_err()
        );
    }
    assert!(DeviceRegistry::new(vec![device("X"), device("X")]).is_err());
    let mut bounded = context();
    bounded.limits.max_nodes = 1;
    assert!(
        compose(
            &registry,
            &[GraphFragment {
                id: "bounded",
                document: &doc,
                device_references: &[reference("ref", "X", false, true)],
                source_references: &[],
                output_references: &[],
            }],
            1,
            bounded,
            &compiler
        )
        .is_err()
    );
    assert_ne!(
        runtime_node_id("A:n", &NodeId::from("B")),
        runtime_node_id("A", &NodeId::from("n:B"))
    );
}

#[test]
fn one_native_microphone_fans_out_to_independent_graph_gains_and_survives_reference_removal() {
    let registry = mic_registry(vec![device("X"), device("Y")]);
    let doc = GraphDocument::new(
        1,
        vec![],
        vec![edge("mic-out", "mic", AUDIO_PORT_ID, "out", TO_OS_PORT_ID)],
    );
    let a_devices = [reference("out", "X", false, true)];
    let b_devices = [reference("out", "Y", false, true)];
    let a_sources = [mic_reference(0.0)];
    let b_sources = [mic_reference(-6.020_6)];
    let a = GraphFragment {
        id: "A",
        document: &doc,
        device_references: &a_devices,
        source_references: &a_sources,
        output_references: &[],
    };
    let b = GraphFragment {
        id: "B",
        document: &doc,
        device_references: &b_devices,
        source_references: &b_sources,
        output_references: &[],
    };
    let both = compose(&registry, &[a, b], 2, context(), &GraphCompiler::default()).unwrap();
    assert_eq!(both.native_sources.len(), 1);
    assert_eq!(
        both.document
            .nodes
            .iter()
            .filter(|node| matches!(node.config, NodeConfig::AudioDeviceInput(_)))
            .count(),
        1
    );
    let out = pcm(&both);
    assert_frame(
        &out[&both.native_devices[&VirtualEndpointId::from("X")]],
        [0.2, 0.4],
    );
    assert_frame(
        &out[&both.native_devices[&VirtualEndpointId::from("Y")]],
        [0.1, 0.2],
    );
    let remaining = compose(
        &registry,
        &[GraphFragment {
            id: "B",
            document: &doc,
            device_references: &b_devices,
            source_references: &b_sources,
            output_references: &[],
        }],
        3,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    assert_eq!(remaining.native_sources, both.native_sources);
    assert_eq!(remaining.native_devices.len(), 1);
    assert_eq!(
        remaining.native_devices[&VirtualEndpointId::from("Y")],
        both.native_devices[&VirtualEndpointId::from("Y")]
    );
    assert!(
        !remaining
            .native_devices
            .contains_key(&VirtualEndpointId::from("X"))
    );
    assert_frame(
        &pcm(&remaining)[&remaining.native_devices[&VirtualEndpointId::from("Y")]],
        [0.1, 0.2],
    );
}

#[test]
fn one_native_application_capture_owner_has_independent_local_reference_gains() {
    let registry = DeviceRegistry::new(vec![device("X"), device("Y")])
        .unwrap()
        .with_sources(vec![SharedSource {
            id: "application:stable-bundle".into(),
            config: NodeConfig::ApplicationCapture(
                mixrelay_audio_graph::ApplicationCaptureRequest {
                    selector: mixrelay_audio_graph::ApplicationSelector::BundleId {
                        bundle_id: "org.example.Player".into(),
                        include_descendants: true,
                    },
                    stream: StreamSpec {
                        channels: 2,
                        channel_layout: ChannelLayout::Stereo,
                    },
                    source_playback: mixrelay_audio_graph::SourcePlaybackPolicy::Preserve,
                    forwarding_endpoint: None,
                    gain_db: 0.0,
                    requires_reselection: false,
                },
            ),
        }])
        .unwrap();
    let doc = GraphDocument::new(
        1,
        vec![],
        vec![edge("app-out", "app", AUDIO_PORT_ID, "out", TO_OS_PORT_ID)],
    );
    let make_source = |gain_db| SourceReference {
        control: mixrelay_audio_graph::NodeControl::default(),
        node_id: NodeId::from("app"),
        source_id: "application:stable-bundle".into(),
        channels: 2,
        gain_db,
        forwarding_endpoint: None,
    };
    let composed = compose(
        &registry,
        &[
            GraphFragment {
                id: "A",
                document: &doc,
                device_references: &[reference("out", "X", false, true)],
                source_references: &[make_source(0.0)],
                output_references: &[],
            },
            GraphFragment {
                id: "B",
                document: &doc,
                device_references: &[reference("out", "Y", false, true)],
                source_references: &[make_source(-6.020_6)],
                output_references: &[],
            },
        ],
        2,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    assert_eq!(
        composed
            .document
            .nodes
            .iter()
            .filter(|node| matches!(node.config, NodeConfig::ApplicationCapture(_)))
            .count(),
        1
    );
    let out = pcm(&composed);
    assert_frame(
        &out[&composed.native_devices[&VirtualEndpointId::from("X")]],
        [0.2, 0.4],
    );
    assert_frame(
        &out[&composed.native_devices[&VirtualEndpointId::from("Y")]],
        [0.1, 0.2],
    );
}

#[test]
fn source_registry_rejects_unknown_channels_and_embedded_duplicate_capture_owners() {
    let registry = mic_registry(vec![]);
    let empty = GraphDocument::new(1, vec![], vec![]);
    let mut invalid_source = mic_reference(0.0);
    invalid_source.channels = 3;
    assert!(
        compose(
            &registry,
            &[GraphFragment {
                id: "A",
                document: &empty,
                device_references: &[],
                source_references: &[invalid_source],
                output_references: &[],
            }],
            1,
            context(),
            &GraphCompiler::default()
        )
        .is_err()
    );
    let embedded = GraphDocument::new(
        1,
        vec![NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("mic"),
            label: None,
            config: mic_config(),
        }],
        vec![],
    );
    assert!(
        compose(
            &registry,
            &[GraphFragment {
                id: "A",
                document: &embedded,
                device_references: &[],
                source_references: &[],
                output_references: &[],
            }],
            1,
            context(),
            &GraphCompiler::default()
        )
        .is_err()
    );
}

fn mic_config() -> NodeConfig {
    NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
        selector: DeviceSelector::PersistentUid {
            uid: "macbook-mic".into(),
        },
        output_channels: 2,
        gain_db: 0.0,
    })
}
fn endpoint(node_id: NodeId, port: &str) -> AudioEndpoint {
    AudioEndpoint {
        node_id,
        port_id: PortId::from(port),
    }
}
fn device_node_id(id: &VirtualEndpointId, side: &str) -> NodeId {
    NodeId::new(format!("d:{}:{}:{side}", id.as_str().len(), id.as_str()))
}
fn reference_node_id(graph: &str, node: &NodeId, side: &str) -> NodeId {
    NodeId::new(format!(
        "g:{}:{graph}:r:{}:{node}:{side}",
        graph.len(),
        node.as_str().len()
    ))
}
