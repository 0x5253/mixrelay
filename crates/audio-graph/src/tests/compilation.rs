//! Graph ordering, channel routes and bounded compilation contracts.
use super::*;

#[test]
fn compiles_fixture_into_a_deterministic_plan() {
    let document: GraphDocument =
        serde_json::from_str(include_str!("../fixtures/stereo-stream.json"))
            .expect("fixture should deserialize");

    let plan = GraphCompiler::default()
        .compile(&document, context())
        .expect("fixture should compile");

    assert_eq!(plan.revision(), 7);
    assert_eq!(plan.format().sample_rate, 96_000);
    assert_eq!(plan.format().block_frames, 256);
    assert_eq!(plan.edges().len(), 5);
    let owned_memory_bytes = plan.owned_memory_bytes();
    assert!(owned_memory_bytes > 0);
    eprintln!("fixture plan reserved bytes: {owned_memory_bytes}");
    assert!(plan.edges().iter().all(|edge| edge.routes().len() == 2));

    let first_edge = &plan.edges()[0];
    assert_eq!(first_edge.id().as_str(), "spat-to-mix");
    assert_eq!(first_edge.source().node_id.as_str(), "spat-ingress");
    assert_eq!(first_edge.source().port_id.as_str(), FROM_OS_PORT_ID);
    assert_eq!(first_edge.destination().node_id.as_str(), "broadcast-mix");

    let ordered_ids: Vec<_> = plan.nodes().iter().map(|node| node.id().as_str()).collect();
    assert_eq!(
        ordered_ids,
        [
            "broadcast-mix",
            "chrome",
            "microphone",
            "mixrelay-stream",
            "monitor",
            "spat-ingress"
        ]
    );
    let chrome = plan
        .nodes()
        .iter()
        .find(|node| node.id().as_str() == "chrome")
        .expect("Chrome source should compile");
    assert_eq!(chrome.kind(), NodeKind::ApplicationCapture);
    assert!(matches!(chrome.config(), NodeConfig::ApplicationCapture(_)));
    assert!(chrome.inputs().is_empty());
    assert_eq!(chrome.outputs()[0].direction(), PortDirection::Output);
    let spat = plan
        .nodes()
        .iter()
        .find(|node| node.id().as_str() == "spat-ingress")
        .expect("SPAT endpoint should compile");
    assert_eq!(spat.outputs()[0].id().as_str(), FROM_OS_PORT_ID);
    assert_eq!(spat.outputs()[0].channels(), 64);
    let obs = plan
        .nodes()
        .iter()
        .find(|node| node.id().as_str() == "mixrelay-stream")
        .expect("OBS endpoint should compile");
    assert_eq!(obs.inputs()[0].id().as_str(), TO_OS_PORT_ID);
    assert_eq!(obs.inputs()[0].channels(), 2);
}

#[test]
fn rejects_cycles_before_the_runtime_sees_them() {
    let acyclic = GraphCompiler::default()
        .compile(
            &GraphDocument::new(
                1,
                vec![gain_node("first"), gain_node("second")],
                vec![edge("first-to-second", "first", "second")],
            ),
            context(),
        )
        .expect("processor dependency should order the destination");
    assert_eq!(
        acyclic
            .nodes()
            .iter()
            .map(|node| node.id().as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let converging = GraphCompiler::default()
        .compile(
            &GraphDocument::new(
                2,
                vec![
                    gain_node("first"),
                    gain_node("second"),
                    configured_node(
                        "mix",
                        NodeConfig::Mixer(MixerConfig {
                            inputs: vec![PortId::from("left"), PortId::from("right")],
                            channels: 2,
                        }),
                    ),
                ],
                vec![
                    EdgeSpec {
                        id: EdgeId::from("first-to-left"),
                        source: endpoint("first"),
                        destination: AudioEndpoint {
                            node_id: NodeId::from("mix"),
                            port_id: PortId::from("left"),
                        },
                        channel_mapping: ChannelMapping::Auto,
                    },
                    EdgeSpec {
                        id: EdgeId::from("second-to-right"),
                        source: endpoint("second"),
                        destination: AudioEndpoint {
                            node_id: NodeId::from("mix"),
                            port_id: PortId::from("right"),
                        },
                        channel_mapping: ChannelMapping::Auto,
                    },
                ],
            ),
            context(),
        )
        .expect("multiple processor dependencies should converge");
    assert_eq!(
        converging.nodes().last().map(|node| node.id().as_str()),
        Some("mix")
    );

    let document = GraphDocument::new(
        1,
        vec![gain_node("first"), gain_node("second")],
        vec![
            edge("first-to-second", "first", "second"),
            edge("second-to-first", "second", "first"),
        ],
    );

    let error = GraphCompiler::default()
        .compile(&document, context())
        .expect_err("cycle must be rejected");

    assert!(matches!(
        error.issues(),
        [GraphIssue::CycleDetected { node_ids }] if node_ids == &vec![NodeId::from("first"), NodeId::from("second")]
    ));
}

#[test]
fn allows_only_virtual_endpoint_pass_through_as_a_self_connection() {
    let endpoint_node = configured_node(
        "virtual",
        virtual_endpoint("virtual", "Virtual", Some(2), Some(2)),
    );
    let pass_through = EdgeSpec {
        id: EdgeId::from("pass-through"),
        source: AudioEndpoint {
            node_id: NodeId::from("virtual"),
            port_id: PortId::from(FROM_OS_PORT_ID),
        },
        destination: AudioEndpoint {
            node_id: NodeId::from("virtual"),
            port_id: PortId::from(TO_OS_PORT_ID),
        },
        channel_mapping: ChannelMapping::Auto,
    };
    let plan = GraphCompiler::default()
        .compile(
            &GraphDocument::new(1, vec![endpoint_node], vec![pass_through]),
            context(),
        )
        .expect("virtual endpoint pass-through is not a feedback cycle");
    assert_eq!(plan.edges().len(), 1);

    let gain_path = GraphDocument::new(
        2,
        vec![
            configured_node(
                "duplex",
                virtual_endpoint("duplex", "Duplex", Some(2), Some(2)),
            ),
            gain_node("gain"),
        ],
        vec![
            EdgeSpec {
                id: EdgeId::from("from-os-to-gain"),
                source: AudioEndpoint {
                    node_id: NodeId::from("duplex"),
                    port_id: PortId::from(FROM_OS_PORT_ID),
                },
                destination: endpoint("gain"),
                channel_mapping: ChannelMapping::Auto,
            },
            EdgeSpec {
                id: EdgeId::from("gain-to-os"),
                source: endpoint("gain"),
                destination: AudioEndpoint {
                    node_id: NodeId::from("duplex"),
                    port_id: PortId::from(TO_OS_PORT_ID),
                },
                channel_mapping: ChannelMapping::Auto,
            },
        ],
    );
    assert!(
        GraphCompiler::default()
            .compile(&gain_path, context())
            .is_ok()
    );

    let error = compile_error(&GraphDocument::new(
        1,
        vec![gain_node("gain")],
        vec![edge("feedback", "gain", "gain")],
    ));
    assert!(matches!(
        error.issues(),
        [GraphIssue::InvalidSelfConnection { edge_id, node_id }]
            if edge_id.as_str() == "feedback" && node_id.as_str() == "gain"
    ));
}

#[test]
fn reports_stable_machine_readable_issue_codes() {
    let document = GraphDocument::new(
        1,
        vec![gain_node("source"), gain_node("destination")],
        vec![EdgeSpec {
            id: EdgeId::from("bad-direction"),
            source: endpoint("source"),
            destination: AudioEndpoint {
                node_id: NodeId::from("destination"),
                port_id: PortId::from("missing"),
            },
            channel_mapping: ChannelMapping::Auto,
        }],
    );

    let error = GraphCompiler::default()
        .compile(&document, context())
        .expect_err("unknown port must be rejected");
    assert_eq!(error.issues()[0].code(), GraphIssueCode::UnknownPort);

    let json = serde_json::to_value(&error.issues()[0]).expect("issue should serialize");
    assert_eq!(json["code"], "unknown_port");
}

#[test]
fn document_validation_aggregates_schema_format_limit_and_id_issues() {
    let mut document = GraphDocument::new(
        1,
        vec![gain_node(""), gain_node("same"), gain_node("same")],
        vec![edge("", "missing", "missing")],
    );
    document.schema_version = 99;
    let invalid_context = CompileContext {
        format: RenderFormat {
            sample_rate: 0,
            block_frames: 0,
        },
        limits: CompileLimits {
            max_nodes: 0,
            max_edges: 0,
            max_channels_per_port: 64,
            max_sample_rate: 384_000,
            max_block_frames: 4_096,
            max_plan_bytes: 8 * 1_024 * 1_024,
        },
    };

    let error = GraphCompiler::default()
        .compile(&document, invalid_context)
        .expect_err("invalid document should fail");
    let codes = issue_codes(&error);
    for expected in [
        GraphIssueCode::UnsupportedSchemaVersion,
        GraphIssueCode::InvalidRenderFormat,
        GraphIssueCode::LimitExceeded,
        GraphIssueCode::InvalidNodeId,
        GraphIssueCode::DuplicateNodeId,
        GraphIssueCode::InvalidEdgeId,
    ] {
        assert!(codes.contains(&expected), "missing {expected:?}");
    }
    assert_eq!(
        error.to_string(),
        "graph compilation failed with 7 issue(s)"
    );
}

#[test]
fn rejects_unbounded_render_formats_and_compiled_plan_memory() {
    for format in [
        RenderFormat {
            sample_rate: 0,
            block_frames: 128,
        },
        RenderFormat {
            sample_rate: 384_001,
            block_frames: 128,
        },
        RenderFormat {
            sample_rate: 48_000,
            block_frames: 0,
        },
        RenderFormat {
            sample_rate: 48_000,
            block_frames: 4_097,
        },
    ] {
        let invalid_context = CompileContext {
            format,
            limits: CompileLimits::default(),
        };
        let error = GraphCompiler::default()
            .compile(&GraphDocument::new(1, vec![], vec![]), invalid_context)
            .expect_err("an unbounded render format must fail");
        assert_eq!(
            error.issues()[0].code(),
            GraphIssueCode::InvalidRenderFormat
        );
    }

    let mut memory_limited_context = context();
    memory_limited_context.limits.max_plan_bytes = 0;
    let error = GraphCompiler::default()
        .compile(
            &GraphDocument::new(1, vec![], vec![]),
            memory_limited_context,
        )
        .expect_err("a plan above its memory budget must fail");
    assert!(matches!(
        error.issues(),
        [GraphIssue::LimitExceeded {
            resource: GraphResource::PlanBytes,
            limit: 0,
            actual,
        }] if *actual > 0
    ));
}

#[test]
fn edge_validation_reports_ids_nodes_ports_and_directions_while_allowing_fan_in() {
    let nodes = vec![
        gain_node("source-a"),
        gain_node("source-b"),
        gain_node("destination"),
        configured_node(
            "sink",
            NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: DeviceSelector::DefaultOutput,
                input_channels: 2,
            }),
        ),
        configured_node(
            "capture",
            app(ApplicationSelector::AllExcept { bundle_ids: vec![] }),
        ),
    ];
    let edges = vec![
        edge("", "source-a", "destination"),
        edge("duplicate", "source-a", "destination"),
        edge("duplicate", "source-b", "destination"),
        edge("unknown-nodes", "missing-a", "missing-b"),
        edge("wrong-direction", "sink", "capture"),
        edge("unknown-port", "source-a", "destination").with_destination_port("missing"),
        edge("fan-in", "source-b", "destination"),
    ];
    let codes = issue_codes(&compile_error(&GraphDocument::new(1, nodes, edges)));

    for expected in [
        GraphIssueCode::InvalidEdgeId,
        GraphIssueCode::DuplicateEdgeId,
        GraphIssueCode::UnknownNode,
        GraphIssueCode::InvalidPortDirection,
        GraphIssueCode::UnknownPort,
    ] {
        assert!(codes.contains(&expected), "missing {expected:?}");
    }
    assert!(!codes.contains(&GraphIssueCode::InputAlreadyConnected));
}

trait EdgeTestExt {
    fn with_destination_port(self, port_id: &str) -> Self;
}

impl EdgeTestExt for EdgeSpec {
    fn with_destination_port(mut self, port_id: &str) -> Self {
        self.destination.port_id = PortId::from(port_id);
        self
    }
}

#[test]
fn channel_mapping_supports_explicit_routes_and_rejects_invalid_mappings() {
    let valid_mapping = ChannelMapping::Explicit {
        routes: vec![ChannelRoute {
            source_channel: 1,
            destination_channel: 0,
        }],
    };
    let valid = GraphDocument::new(
        1,
        vec![gain_node("source"), gain_node("destination")],
        vec![EdgeSpec {
            channel_mapping: valid_mapping,
            ..edge("mapped", "source", "destination")
        }],
    );
    assert_eq!(
        GraphCompiler::default()
            .compile(&valid, context())
            .expect("explicit mapping should compile")
            .edges()[0]
            .routes(),
        [ChannelRoute {
            source_channel: 1,
            destination_channel: 0,
        }]
    );

    assert_invalid_mapping(
        ChannelMapping::Auto,
        1,
        2,
        GraphIssueCode::ChannelCountMismatch,
    );
    assert_invalid_mapping(
        ChannelMapping::Explicit { routes: vec![] },
        2,
        2,
        GraphIssueCode::EmptyChannelMapping,
    );
    let bad_routes = vec![
        ChannelRoute {
            source_channel: 2,
            destination_channel: 0,
        },
        ChannelRoute {
            source_channel: 0,
            destination_channel: 2,
        },
        ChannelRoute {
            source_channel: 0,
            destination_channel: 0,
        },
        ChannelRoute {
            source_channel: 0,
            destination_channel: 0,
        },
    ];
    assert_invalid_mapping(
        ChannelMapping::Explicit { routes: bad_routes },
        2,
        2,
        GraphIssueCode::InvalidChannelRoute,
    );
}

fn assert_invalid_mapping(
    mapping: ChannelMapping,
    source_channels: u16,
    destination_channels: u16,
    expected: GraphIssueCode,
) {
    let document = GraphDocument::new(
        1,
        vec![
            configured_node("source", gain(source_channels, 0.0)),
            configured_node("destination", gain(destination_channels, 0.0)),
        ],
        vec![EdgeSpec {
            channel_mapping: mapping,
            ..edge("mapping", "source", "destination")
        }],
    );
    assert!(issue_codes(&compile_error(&document)).contains(&expected));
}
