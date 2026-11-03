#![allow(clippy::float_cmp)] // Exact detector ownership and dry recovery contracts.
use super::*;

const HISTORY_FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: 64,
};

fn dynamics(kind: &str, channels: u16) -> NodeConfig {
    serde_json::from_value(serde_json::json!({
        "type": kind,
        "config": if kind == "compressor" {
            serde_json::json!({"version":1,"bypass":false,"channels":channels,"thresholdDb":-18,
                "ratio":4,"attackMs":0.1,"releaseMs":100,"makeupGainDb":0})
        } else {
            serde_json::json!({"version":1,"bypass":false,"channels":channels,"ceilingDb":-6,"releaseMs":100})
        },
    }))
    .unwrap()
}

fn input(id: &str, channels: u16) -> NodeSpec {
    node(
        id,
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::DefaultInput,
            output_channels: channels,
            gain_db: 0.0,
        }),
    )
}

fn output(id: &str, channels: u16) -> NodeSpec {
    node(
        id,
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: channels,
        }),
    )
}

fn prepare_graph(graph: &GraphDocument, format: RenderFormat) -> Box<BlockProcessor> {
    let execution = GraphCompiler::default()
        .compile(
            graph,
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

fn prepared(
    config: NodeConfig,
    revision: u64,
    source: &str,
    upstream: bool,
    format: RenderFormat,
) -> Box<BlockProcessor> {
    let channels = match &config {
        NodeConfig::Compressor(config) => config.channels,
        NodeConfig::Limiter(config) => config.channels,
        _ => unreachable!("test prepares only dynamics nodes"),
    };
    let mut nodes = vec![
        input(source, channels),
        node("dynamics", config),
        output("sink", channels),
    ];
    let mut edges = vec![edge(
        "out",
        "dynamics",
        AUDIO_PORT_ID,
        "sink",
        AUDIO_PORT_ID,
    )];
    if upstream {
        nodes.push(node(
            "gain",
            NodeConfig::Gain(GainConfig {
                channels,
                gain_db: 0.0,
            }),
        ));
        edges.push(edge(
            "upstream",
            source,
            AUDIO_PORT_ID,
            "gain",
            AUDIO_PORT_ID,
        ));
        edges.push(edge("in", "gain", AUDIO_PORT_ID, "dynamics", AUDIO_PORT_ID));
    } else {
        edges.push(edge("in", source, AUDIO_PORT_ID, "dynamics", AUDIO_PORT_ID));
    }
    prepare_graph(&GraphDocument::new(revision, nodes, edges), format)
}

fn process_constant(processor: &mut BlockProcessor, value: f32) -> f32 {
    let channels = processor.plan.source_bindings[0].channels;
    let frames = processor.plan.format.block_frames;
    let samples = sample_count(frames as usize, channels);
    let input = [value; 256];
    let mut output = [0.0; 256];
    processor
        .process_block(frames, &[&input[..samples]], &mut [&mut output[..samples]])
        .unwrap();
    output[0]
}

fn warm(processor: &mut BlockProcessor) {
    for _ in 0..8 {
        process_constant(processor, 1.0);
    }
    assert!(processor.dynamics_envelopes[0] > 0.99);
}

fn identity_source(kind: usize, changed: bool) -> NodeSpec {
    let identity = if changed { "other" } else { "original" };
    match kind {
        0 => node(
            "source",
            NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::PersistentUid {
                    uid: identity.into(),
                },
                output_channels: 2,
                gain_db: 0.0,
            }),
        ),
        1 | 2 => {
            let mut source = app_source("source");
            let NodeConfig::ApplicationCapture(config) = &mut source.config else {
                unreachable!();
            };
            if kind == 1 {
                config.selector = ApplicationSelector::BundleId {
                    bundle_id: format!("test.{identity}"),
                    include_descendants: true,
                };
            } else if changed {
                config.stream.channel_layout = ChannelLayout::Discrete;
            }
            source
        }
        _ => {
            let mut source = virtual_endpoint("source");
            let NodeConfig::VirtualEndpointTransport(spec) = &mut source.config else {
                unreachable!();
            };
            spec.id = VirtualEndpointId::from(identity);
            spec.render = None;
            source
        }
    }
}

fn identity_graph(source: NodeSpec, middle: NodeConfig) -> GraphDocument {
    let source_port = if matches!(&source.config, NodeConfig::VirtualEndpointTransport(_)) {
        FROM_OS_PORT_ID
    } else {
        AUDIO_PORT_ID
    };
    GraphDocument::new(
        1,
        vec![source, node("middle", middle), output("sink", 2)],
        vec![
            edge("in", "source", source_port, "middle", AUDIO_PORT_ID),
            edge("out", "middle", AUDIO_PORT_ID, "sink", AUDIO_PORT_ID),
        ],
    )
}

#[test]
fn replacing_source_identity_with_unchanged_route_ids_resets_detectors_and_effect_tails() {
    let delay = builtin_effects::configs()
        .into_iter()
        .find(|config| matches!(config.effect, mixrelay_audio_graph::Effect::Delay(_)))
        .unwrap();
    for kind in 0..4 {
        for middle in [dynamics("compressor", 2), NodeConfig::Effect(delay.clone())] {
            let original = identity_graph(identity_source(kind, false), middle.clone());
            let mut current = prepare_graph(&original, HISTORY_FORMAT);
            for _ in 0..8 {
                process_constant(&mut current, 1.0);
            }
            let mut edited = identity_graph(identity_source(kind, true), middle);
            edited.revision = 2;
            let exchange = crate::ProcessorExchange::new(1);
            exchange
                .submit(prepare_graph(&edited, HISTORY_FORMAT))
                .unwrap();
            HEAP_OPERATIONS.with(|count| count.set(Some(0)));
            assert!(exchange.activate_pending(&mut current));
            assert_eq!(process_constant(&mut current, 0.0), 0.0);
            assert!(current.dynamics_envelopes.iter().all(|value| *value == 0.0));
            assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
            exchange.reclaim_retired();
        }
    }
}

#[test]
fn bus_retains_the_same_signal_but_discards_changed_sources_and_feeding_routes() {
    for change in 0..3 {
        let original = identity_graph(
            identity_source(0, false),
            NodeConfig::DeviceBus { channels: 2 },
        );
        let mut previous = prepare_graph(&original, HISTORY_FORMAT);
        process_constant(&mut previous, 0.25);
        let mut edited = original;
        match change {
            0 => {
                let NodeConfig::AudioDeviceInput(config) = &mut edited.nodes[0].config else {
                    unreachable!();
                };
                config.gain_db = -6.0;
                edited.nodes[0].label = Some("Renamed source".into());
            }
            1 => edited.nodes[0] = identity_source(0, true),
            _ => {
                edited.nodes[0] = identity_source(1, false);
                edited.nodes[0].id = NodeId::from("new-source");
                edited.edges[0].source.node_id = NodeId::from("new-source");
            }
        }
        let mut next = prepare_graph(&edited, HISTORY_FORMAT);
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        next.retain_device_bus_state(&previous);
        assert_eq!(
            process_constant(&mut next, 0.0),
            if change == 0 { 0.25 } else { 0.0 }
        );
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    }
}

#[test]
fn parameter_edits_retain_detector_and_apply_new_targets_without_callback_allocation() {
    for kind in ["compressor", "limiter"] {
        let config = dynamics(kind, 2);
        let mut current = prepared(config.clone(), 1, "source", false, HISTORY_FORMAT);
        warm(&mut current);
        let envelope = current.dynamics_envelopes[0];
        let wet = current.dynamics_wet[0];
        let mut edited = config;
        match &mut edited {
            NodeConfig::Compressor(config) => {
                config.threshold_db = -24.0;
                config.ratio = 6.0;
                config.attack_ms = 1.0;
                config.release_ms = 200.0;
            }
            NodeConfig::Limiter(config) => {
                config.ceiling_db = -12.0;
                config.release_ms = 200.0;
            }
            _ => unreachable!(),
        }
        let mut fresh = prepared(edited.clone(), 2, "source", false, HISTORY_FORMAT);
        let fresh_output = process_constant(&mut fresh, 0.1);
        let exchange = crate::ProcessorExchange::new(1);
        exchange
            .submit(prepared(edited, 2, "source", false, HISTORY_FORMAT))
            .unwrap();
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        assert!(exchange.activate_pending(&mut current));
        assert_eq!(current.dynamics_envelopes[0], envelope);
        assert_eq!(current.dynamics_wet[0], wet);
        let resumed = process_constant(&mut current, 0.1);
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert!(resumed > 0.0 && resumed < 0.05);
        assert!(fresh_output > resumed * 2.0);
        exchange.reclaim_retired();
    }
}

#[test]
fn direct_and_upstream_reroutes_start_fresh_dynamics_history() {
    for kind in ["compressor", "limiter"] {
        for upstream in [false, true] {
            let config = dynamics(kind, 2);
            let mut current = prepared(config.clone(), 1, "source-a", upstream, HISTORY_FORMAT);
            warm(&mut current);
            let exchange = crate::ProcessorExchange::new(1);
            exchange
                .submit(prepared(config, 2, "source-b", upstream, HISTORY_FORMAT))
                .unwrap();
            HEAP_OPERATIONS.with(|count| count.set(Some(0)));
            assert!(exchange.activate_pending(&mut current));
            assert_eq!(current.dynamics_envelopes[0], 0.0);
            assert_eq!(process_constant(&mut current, 0.1), 0.1);
            assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
            exchange.reclaim_retired();
        }
    }
}

#[test]
fn incompatible_kind_channels_and_format_replacements_do_not_adopt_detectors() {
    for kind in ["compressor", "limiter"] {
        for replacement in 0..3 {
            let mut current = prepared(dynamics(kind, 2), 1, "source", false, HISTORY_FORMAT);
            warm(&mut current);
            let (new_kind, channels, format) = match replacement {
                0 => (
                    if kind == "compressor" {
                        "limiter"
                    } else {
                        "compressor"
                    },
                    2,
                    HISTORY_FORMAT,
                ),
                1 => (kind, 1, HISTORY_FORMAT),
                _ => (
                    kind,
                    2,
                    RenderFormat {
                        sample_rate: 44_100,
                        ..HISTORY_FORMAT
                    },
                ),
            };
            let exchange = crate::ProcessorExchange::new(1);
            exchange
                .submit(prepared(
                    dynamics(new_kind, channels),
                    2,
                    "source",
                    false,
                    format,
                ))
                .unwrap();
            HEAP_OPERATIONS.with(|count| count.set(Some(0)));
            assert!(exchange.activate_pending(&mut current));
            assert_eq!(current.dynamics_envelopes[0], 0.0);
            assert_eq!(process_constant(&mut current, 0.1), 0.1);
            assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
            exchange.reclaim_retired();
        }
    }
}

struct BranchIo {
    value: f32,
    restart_a: bool,
    admission: CycleAdmission,
    output_a: [f32; 128],
    output_b: [f32; 128],
}

impl ExternalIo for BranchIo {
    fn begin_cycle(
        &mut self,
        _cycle: &ProcessCycle,
        _controls: ExternalControlState<'_>,
    ) -> CycleAdmission {
        self.admission
    }

    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        destination.fill(self.value);
        TransferOutcome {
            discontinuity: self.restart_a && binding.node_id().as_str() == "source-a",
            ..TransferOutcome::complete(cycle.frames)
        }
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        let destination = match binding.node_id().as_str() {
            "sink-a" => &mut self.output_a,
            "sink-b" => &mut self.output_b,
            other => panic!("unexpected sink {other}"),
        };
        destination[..source.len()].copy_from_slice(source);
        TransferOutcome::complete(cycle.frames)
    }
}

fn branches() -> (Box<BlockProcessor>, BranchIo, ProcessCycle) {
    let processor = prepare_graph(
        &GraphDocument::new(
            1,
            vec![
                input("source-a", 2),
                node("z-compressor", dynamics("compressor", 2)),
                output("sink-a", 2),
                input("source-b", 2),
                node("a-limiter", dynamics("limiter", 2)),
                output("sink-b", 2),
            ],
            vec![
                edge(
                    "in-a",
                    "source-a",
                    AUDIO_PORT_ID,
                    "z-compressor",
                    AUDIO_PORT_ID,
                ),
                edge(
                    "out-a",
                    "z-compressor",
                    AUDIO_PORT_ID,
                    "sink-a",
                    AUDIO_PORT_ID,
                ),
                edge(
                    "in-b",
                    "source-b",
                    AUDIO_PORT_ID,
                    "a-limiter",
                    AUDIO_PORT_ID,
                ),
                edge("out-b", "a-limiter", AUDIO_PORT_ID, "sink-b", AUDIO_PORT_ID),
            ],
        ),
        HISTORY_FORMAT,
    );
    let mut result = (
        processor,
        BranchIo {
            value: 1.0,
            restart_a: false,
            admission: CycleAdmission::Process,
            output_a: [0.0; 128],
            output_b: [0.0; 128],
        },
        ProcessCycle {
            clock: mixrelay_audio_io::process::ClockDomain {
                authority: mixrelay_audio_io::session::ClockAuthority::Device,
                id: 1,
                epoch: 0,
            },
            sample_rate: HISTORY_FORMAT.sample_rate,
            frames: HISTORY_FORMAT.block_frames,
            first_frame: 0,
            monotonic_ns: None,
            discontinuity: false,
        },
    );
    for _ in 0..8 {
        result
            .0
            .process_external_cycle(result.2, &mut result.1)
            .unwrap();
        result.2.first_frame += u64::from(result.2.frames);
    }
    assert!(
        result
            .0
            .dynamics_envelopes
            .iter()
            .all(|envelope| *envelope > 0.99)
    );
    result.1.value = 0.1;
    result
}

#[test]
fn local_restart_clears_only_downstream_detector_even_during_lossless_clock_handoff() {
    for handoff in [false, true] {
        let (mut processor, mut io, mut cycle) = branches();
        io.restart_a = true;
        if handoff {
            io.admission = CycleAdmission::ClockHandoff;
            cycle.clock.id = 2;
            cycle.first_frame = 960;
            cycle.discontinuity = true;
        }
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        let report = processor
            .process_external_cycle(cycle, &mut io)
            .unwrap()
            .unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert_eq!(report.discontinuities, 1);
        assert_eq!(io.output_a, [0.1; 128]);
        assert!(io.output_b[0] > 0.0 && io.output_b[0] < 0.06);
        let comp = processor
            .plan
            .dynamics_bindings
            .iter()
            .find(|binding| matches!(binding.config, NodeConfig::Compressor(_)))
            .unwrap();
        let limiter = processor
            .plan
            .dynamics_bindings
            .iter()
            .find(|binding| matches!(binding.config, NodeConfig::Limiter(_)))
            .unwrap();
        assert!(processor.dynamics_envelopes[comp.state_index] <= 0.1);
        assert!(processor.dynamics_envelopes[limiter.state_index] > 0.9);
    }
}

#[test]
fn global_clock_restart_clears_both_detectors_and_restores_healthy_quiet_audio() {
    let (mut processor, mut io, mut cycle) = branches();
    cycle.clock.epoch += 1;
    cycle.discontinuity = true;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(io.output_a, [0.1; 128]);
    assert_eq!(io.output_b, [0.1; 128]);
    assert!(
        processor
            .dynamics_envelopes
            .iter()
            .all(|envelope| *envelope <= 0.1)
    );
}
