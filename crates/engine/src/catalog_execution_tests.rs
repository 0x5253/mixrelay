use super::*;
use mixrelay_audio_engine::{BlockProcessor, RenderPlan};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioEndpoint, ChannelMapping, CompileLimits, EdgeId, EdgeSpec, NodeId,
    RenderFormat, VirtualEndpointConfig, VirtualEndpointReference,
};
use mixrelay_audio_io::{ApplicationSelector, ChannelLayout, SourcePlaybackPolicy, StreamSpec};
use std::{cell::Cell, sync::Arc};

struct Apps(Cell<usize>);
impl ApplicationIdentityResolver for Apps {
    fn resolve(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<SharedSource, CompositionError> {
        self.0.set(self.0.get() + 1);
        if request.requires_reselection
            || matches!(&request.selector,
            ApplicationSelector::ExecutablePath { path } if path == "/unavailable")
        {
            return Err(invalid("application requires native reselection"));
        }
        let mut canonical = request.clone();
        canonical.selector = ApplicationSelector::BundleId {
            bundle_id: "verified.app".into(),
            include_descendants: true,
        };
        canonical.gain_db = 0.0;
        Ok(SharedSource {
            id: "verified-native-app".into(),
            config: NodeConfig::ApplicationCapture(canonical),
        })
    }
}
fn inventory() -> Vec<AudioDeviceDescriptor> {
    vec![AudioDeviceDescriptor {
        uid: "physical-one".into(),
        display_name: "Hardware".into(),
        input_channels: 2,
        output_channels: 2,
        is_default_input: true,
        is_default_output: true,
    }]
}
fn compile(
    documents: &[(String, GraphDocument)],
    devices: &[AudioDeviceDescriptor],
    apps: &Apps,
) -> Result<ComposedGraph, CompositionError> {
    compose_catalog(
        documents,
        CatalogExecutionContext {
            revision: 9,
            compile: CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 4,
                },
                limits: CompileLimits::default(),
            },
            compiler: &GraphCompiler::default(),
            devices,
            applications: apps,
        },
    )
}
fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: id.into(),
        label: None,
        config,
    }
}
fn route(id: &str, source: &str, source_port: &str, sink: &str, sink_port: &str) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: AudioEndpoint {
            node_id: source.into(),
            port_id: source_port.into(),
        },
        destination: AudioEndpoint {
            node_id: sink.into(),
            port_id: sink_port.into(),
        },
        channel_mapping: ChannelMapping::default(),
    }
}
fn mic(selector: DeviceSelector, gain_db: f32) -> NodeSpec {
    node(
        "input",
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector,
            output_channels: 2,
            gain_db,
        }),
    )
}
fn output(selector: DeviceSelector) -> NodeSpec {
    node(
        "output",
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector,
            input_channels: 2,
        }),
    )
}
fn physical_graph(selector: DeviceSelector, gain: f32) -> GraphDocument {
    GraphDocument::new(
        1,
        vec![mic(selector, gain), output(DeviceSelector::DefaultOutput)],
        vec![route(
            "link",
            "input",
            AUDIO_PORT_ID,
            "output",
            AUDIO_PORT_ID,
        )],
    )
}
fn device(id: &str, read: bool, capture: bool) -> VirtualEndpointConfig {
    VirtualEndpointConfig {
        spec: VirtualEndpointSpec {
            id: id.into(),
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
        },
        reference: VirtualEndpointReference {
            render: read,
            capture,
        },
        gain_db: 0.0,
    }
}
fn pcm(composed: &ComposedGraph) -> BTreeMap<NodeId, Vec<f32>> {
    let plan = Arc::new(RenderPlan::prepare(&composed.execution).unwrap());
    let sources = plan
        .source_bindings()
        .iter()
        .map(|binding| {
            let sample = if composed
                .native_sources
                .values()
                .any(|id| id == binding.node_id())
            {
                0.25
            } else if composed.native_devices.get(&VirtualEndpointId::from("X"))
                == Some(binding.node_id())
            {
                0.8
            } else {
                0.0
            };
            vec![sample; usize::from(binding.channels()) * 4]
        })
        .collect::<Vec<_>>();
    let mut sinks = plan
        .sink_bindings()
        .iter()
        .map(|binding| vec![0.0; usize::from(binding.channels()) * 4])
        .collect::<Vec<_>>();
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
fn samples_equal(samples: &[f32], value: f32) {
    assert!(
        samples
            .iter()
            .all(|sample| (*sample - value).abs() < 0.00001)
    );
}

#[test]
fn disabled_references_stop_only_the_final_shared_owner_and_mute_stays_local() {
    let apps = Apps(Cell::new(0));
    let mut first = physical_graph(DeviceSelector::DefaultInput, 0.0);
    let second = first.clone();
    first.nodes[0].control.muted = true;
    let mut documents = vec![("a".into(), first), ("b".into(), second)];
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(composed.native_sources.len(), 1);
    assert_eq!(composed.native_outputs.len(), 1);
    for samples in pcm(&composed).values() {
        samples_equal(samples, 0.25);
    }
    documents[0].1.nodes[0].control.enabled = false;
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(composed.native_sources.len(), 1);
    for samples in pcm(&composed).values() {
        samples_equal(samples, 0.25);
    }
    documents[1].1.nodes[0].control.enabled = false;
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(composed.native_sources.len(), 1);
    let owner = composed.native_sources.values().next().unwrap();
    assert!(
        composed
            .document
            .nodes
            .iter()
            .any(|node| &node.id == owner && !node.control.enabled)
    );
    for samples in pcm(&composed).values() {
        samples_equal(samples, 0.0);
    }
    documents[0].1.nodes[1].control.enabled = false;
    documents[1].1.nodes[1].control.enabled = false;
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(composed.native_outputs.len(), 1);
    assert_eq!(composed.document.nodes.len(), 6);
    assert!(
        composed
            .document
            .nodes
            .iter()
            .all(|node| !node.control.enabled)
    );
    assert_eq!(composed.document.edges.len(), 6);
}

#[test]
fn disabled_unknown_processor_does_not_block_composition_or_erase_authored_state() {
    let apps = Apps(Cell::new(0));
    let document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2, "revision":1, "edges":[],
        "nodes":[{"id":"missing","type":"provider","control":{"enabled":false,"muted":true},
            "config":{"typeId":"au:not-installed","parameters":{"saved":42}}}]
    }))
    .unwrap();
    let composed = compile(&[("a".into(), document.clone())], &[], &apps).unwrap();
    assert_eq!(composed.document.nodes.len(), 1);
    assert!(!composed.document.nodes[0].control.enabled);
    assert_eq!(composed.node_origins.len(), 1);
    assert_eq!(
        serde_json::to_value(&document).unwrap()["nodes"][0]["config"]["parameters"]["saved"],
        42
    );
}

#[test]
fn actual_inventory_resolves_default_and_uid_to_one_capture_and_mixed_output() {
    let apps = Apps(Cell::new(0));
    let documents = vec![
        (
            "A".into(),
            physical_graph(DeviceSelector::DefaultInput, 0.0),
        ),
        (
            "B".into(),
            physical_graph(
                DeviceSelector::PersistentUid {
                    uid: "physical-one".into(),
                },
                -6.0206,
            ),
        ),
    ];
    let before = documents.clone();
    let composition = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(documents, before);
    assert_eq!(apps.0.get(), 0);
    assert_eq!(composition.native_sources.len(), 1);
    assert_eq!(composition.native_outputs.len(), 1);
    let output = composition.native_outputs.values().next().unwrap();
    samples_equal(&pcm(&composition)[output], 0.375);
    let remaining = compile(&documents[1..], &inventory(), &apps).unwrap();
    assert_eq!(remaining.native_sources, composition.native_sources);
    assert_eq!(remaining.native_outputs, composition.native_outputs);
    samples_equal(&pcm(&remaining)[output], 0.125);
}

#[test]
fn absent_physical_selection_is_preserved_but_ambiguity_and_wrong_width_are_rejected() {
    let apps = Apps(Cell::new(0));
    let documents = vec![(
        "A".into(),
        physical_graph(DeviceSelector::DefaultInput, 0.0),
    )];
    let offline = compile(&documents, &[], &apps).unwrap();
    assert_eq!(offline.native_sources.len(), 1);
    assert_eq!(offline.native_outputs.len(), 1);
    let mut devices = inventory();
    devices.push(devices[0].clone());
    assert!(compile(&documents, &devices, &apps).is_err());
    devices.pop();
    devices[0].input_channels = 1;
    assert!(compile(&documents, &devices, &apps).is_err());
    assert_eq!(apps.0.get(), 0);
}

#[path = "catalog_offline_tests.rs"]
mod offline_tests;

#[test]
fn conflicting_virtual_definitions_are_rejected_but_local_reference_flags_are_independent() {
    let apps = Apps(Cell::new(0));
    let a = node(
        "virtual",
        NodeConfig::VirtualEndpoint(device("X", false, true)),
    );
    let b = node(
        "virtual",
        NodeConfig::VirtualEndpoint(device("X", true, false)),
    );
    let mut documents = vec![
        ("A".into(), GraphDocument::new(1, vec![a], vec![])),
        ("B".into(), GraphDocument::new(1, vec![b], vec![])),
    ];
    let composed = compile(&documents, &[], &apps).unwrap();
    assert_eq!(composed.native_devices.len(), 1);
    let NodeConfig::VirtualEndpoint(config) = &mut documents[1].1.nodes[0].config else {
        panic!()
    };
    config.spec.display_name = "Conflicting definition".into();
    assert!(compile(&documents, &[], &apps).is_err());
}

#[test]
fn real_catalog_pcm_sends_only_graph_contributions_and_reads_driver_mix() {
    let apps = Apps(Cell::new(0));
    let a = GraphDocument::new(
        1,
        vec![
            mic(DeviceSelector::DefaultInput, 0.0),
            node("out", NodeConfig::VirtualEndpoint(device("X", false, true))),
        ],
        vec![route("wire", "input", AUDIO_PORT_ID, "out", "to_os")],
    );
    let reader = |id: &str| {
        GraphDocument::new(
            1,
            vec![
                node("in", NodeConfig::VirtualEndpoint(device("X", true, false))),
                node("out", NodeConfig::VirtualEndpoint(device(id, false, true))),
            ],
            vec![route("wire", "in", "from_os", "out", "to_os")],
        )
    };
    let documents = vec![
        ("A".into(), a),
        ("B".into(), reader("Y")),
        ("C".into(), reader("Z")),
    ];
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    let output = pcm(&composed);
    samples_equal(
        &output[&composed.native_devices[&VirtualEndpointId::from("X")]],
        0.25,
    );
    samples_equal(
        &output[&composed.native_devices[&VirtualEndpointId::from("Y")]],
        0.8,
    );
    samples_equal(
        &output[&composed.native_devices[&VirtualEndpointId::from("Z")]],
        0.8,
    );
    assert_eq!(composed.native_sources.len(), 1);
    let self_route = compile(&[("loop".into(), reader("X"))], &[], &apps).unwrap();
    samples_equal(
        &pcm(&self_route)[&self_route.native_devices[&VirtualEndpointId::from("X")]],
        0.8,
    );
}

#[test]
fn application_owner_comes_only_from_resolver_and_failures_remain_visible() {
    let apps = Apps(Cell::new(0));
    let request = ApplicationCaptureRequest {
        selector: ApplicationSelector::ExecutablePath {
            path: "/verified/app".into(),
        },
        stream: StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        },
        source_playback: SourcePlaybackPolicy::Preserve,
        forwarding_endpoint: None,
        gain_db: 0.0,
        requires_reselection: false,
    };
    let graph = GraphDocument::new(
        1,
        vec![
            node("app", NodeConfig::ApplicationCapture(request)),
            output(DeviceSelector::DefaultOutput),
        ],
        vec![route("wire", "app", AUDIO_PORT_ID, "output", AUDIO_PORT_ID)],
    );
    let mut documents = vec![("A".into(), graph.clone()), ("B".into(), graph)];
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(apps.0.get(), 2);
    assert_eq!(composed.native_sources.len(), 1);
    let output = composed.native_outputs.values().next().unwrap();
    samples_equal(&pcm(&composed)[output], 0.5);
    let NodeConfig::ApplicationCapture(request) = &mut documents[1].1.nodes[0].config else {
        panic!()
    };
    request.requires_reselection = true;
    let inactive = compile(&documents, &inventory(), &apps).unwrap();
    assert_eq!(apps.0.get(), 3, "inactive source must not be resolved");
    assert_eq!(inactive.native_sources.len(), 1);
    let output = inactive.native_outputs.values().next().unwrap();
    samples_equal(&pcm(&inactive)[output], 0.25);
    let NodeConfig::ApplicationCapture(request) = &mut documents[1].1.nodes[0].config else {
        panic!()
    };
    request.requires_reselection = false;
    request.selector = ApplicationSelector::ExecutablePath {
        path: "/unavailable".into(),
    };
    assert!(compile(&documents, &inventory(), &apps).is_err());
}

#[test]
fn empty_catalog_does_not_resolve_apps_or_create_native_resources() {
    let apps = Apps(Cell::new(0));
    let composed = compile(&[], &[], &apps).unwrap();
    assert!(composed.document.nodes.is_empty());
    assert!(composed.native_devices.is_empty());
    assert!(composed.native_sources.is_empty());
    assert!(composed.native_outputs.is_empty());
    assert_eq!(composed.document.revision, 9);
    assert_eq!(apps.0.get(), 0);
}

#[test]
fn disabled_capture_does_not_acquire_forwarding_endpoint_or_change_authored_policy() {
    let apps = Apps(Cell::new(0));
    let mut app = node(
        "app",
        NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
            selector: ApplicationSelector::ExecutablePath {
                path: "/unavailable".into(),
            },
            stream: StreamSpec {
                channels: 2,
                channel_layout: ChannelLayout::Stereo,
            },
            source_playback: SourcePlaybackPolicy::MuteWhileCaptured,
            forwarding_endpoint: Some(VirtualEndpointId::from("absent")),
            gain_db: 0.0,
            requires_reselection: false,
        }),
    );
    app.control.enabled = false;
    let documents = vec![("A".into(), GraphDocument::new(1, vec![app], vec![]))];
    let original = documents.clone();
    let inactive = compile(&documents, &[], &apps).unwrap();
    assert_eq!(apps.0.get(), 0);
    assert!(inactive.native_devices.is_empty());
    assert!(inactive.native_sources.is_empty());
    assert_eq!(documents, original);
}

#[test]
fn application_widths_share_one_owner_and_incomplete_mute_is_rejected() {
    let apps = Apps(Cell::new(0));
    let graph = |channels, gain_db| {
        GraphDocument::new(
            1,
            vec![node(
                "app",
                NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
                    selector: ApplicationSelector::ExecutablePath {
                        path: "/verified/app".into(),
                    },
                    stream: StreamSpec {
                        channels,
                        channel_layout: ChannelLayout::Discrete,
                    },
                    source_playback: SourcePlaybackPolicy::Preserve,
                    forwarding_endpoint: None,
                    gain_db,
                    requires_reselection: false,
                }),
            )],
            Vec::new(),
        )
    };
    let mut documents = vec![("A".into(), graph(1, -12.0)), ("B".into(), graph(32, 0.0))];
    let composed = compile(&documents, &[], &apps).unwrap();
    assert_eq!(composed.native_sources.len(), 1);
    let source = composed
        .document
        .nodes
        .iter()
        .find_map(|node| match &node.config {
            NodeConfig::ApplicationCapture(request) => Some(request),
            _ => None,
        })
        .unwrap();
    assert_eq!(source.stream.channels, 32);
    assert_eq!(source.gain_db.to_bits(), 0.0_f32.to_bits());
    documents.reverse();
    let reordered = compile(&documents, &[], &apps).unwrap();
    assert_eq!(reordered.native_sources, composed.native_sources);
    assert!(
        reordered
            .document
            .nodes
            .iter()
            .any(|node| { node.config == NodeConfig::ApplicationCapture(source.clone()) })
    );
    let NodeConfig::ApplicationCapture(request) = &mut documents[0].1.nodes[0].config else {
        panic!("capture")
    };
    request.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
    assert!(compile(&documents, &[], &apps).is_err());
}

#[test]
fn application_mute_intent_is_graph_local_to_one_canonical_tap() {
    let apps = Apps(Cell::new(0));
    let request = ApplicationCaptureRequest {
        selector: ApplicationSelector::ExecutablePath {
            path: "/verified/app".into(),
        },
        stream: StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        },
        source_playback: SourcePlaybackPolicy::Preserve,
        forwarding_endpoint: None,
        gain_db: 0.0,
        requires_reselection: false,
    };
    let mut muted = request.clone();
    muted.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
    muted.forwarding_endpoint = Some(VirtualEndpointId::from("X"));
    let documents = vec![
        (
            "A".into(),
            GraphDocument::new(
                1,
                vec![node("app", NodeConfig::ApplicationCapture(request))],
                vec![],
            ),
        ),
        (
            "B".into(),
            GraphDocument::new(
                1,
                vec![
                    node("app", NodeConfig::ApplicationCapture(muted)),
                    node(
                        "endpoint",
                        NodeConfig::VirtualEndpoint(device("X", true, false)),
                    ),
                ],
                vec![],
            ),
        ),
    ];
    let composed = compile(&documents, &[], &apps).unwrap();
    assert_eq!(composed.native_sources.len(), 1);
    assert_eq!(composed.playback_intents.len(), 1);
    let intent = &composed.playback_intents[0];
    assert_eq!(
        composed.native_sources.get(&intent.source_id),
        Some(&intent.capture_node_id)
    );
    assert_eq!(
        intent.mask_node_id,
        crate::graph_composition::runtime_node_id("B", &NodeId::from("app"))
    );
    assert_eq!(intent.forwarding_endpoint, VirtualEndpointId::from("X"));
    assert_eq!(composed.native_devices.len(), 1);
    let canonical = composed
        .document
        .nodes
        .iter()
        .find_map(|node| match &node.config {
            NodeConfig::ApplicationCapture(request) => Some(request),
            _ => None,
        })
        .unwrap();
    assert_eq!(canonical.source_playback, SourcePlaybackPolicy::Preserve);
    assert_eq!(canonical.forwarding_endpoint, None);
}
