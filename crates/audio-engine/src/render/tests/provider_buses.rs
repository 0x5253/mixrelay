use super::*;
use mixrelay_audio_graph::{ProviderBus, ProviderBusKind, ProviderChannelFormat, ProviderPorts};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::RuntimeControlUpdate;

const SIDECHAIN_PORT: &str = "bus:1";

fn provider_layout() -> ProviderPorts {
    ProviderPorts {
        layout_id: "main-stereo-sidechain-mono".into(),
        inputs: vec![
            bus(
                "audio",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            ),
            bus(
                SIDECHAIN_PORT,
                ProviderBusKind::Sidechain,
                1,
                ProviderChannelFormat::Mono,
            ),
        ],
        outputs: vec![
            bus(
                "audio",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            ),
            bus(
                SIDECHAIN_PORT,
                ProviderBusKind::Auxiliary,
                1,
                ProviderChannelFormat::Mono,
            ),
        ],
    }
}

fn bus(
    id: &str,
    kind: ProviderBusKind,
    channels: u16,
    channel_format: ProviderChannelFormat,
) -> ProviderBus {
    ProviderBus {
        id: PortId::from(id),
        name: id.to_owned(),
        kind,
        channels,
        channel_format,
    }
}

struct MultibusResolver;

impl NodeResolver for MultibusResolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        let NodeConfig::Provider(config) = &node.config else {
            return BuiltInNodeResolver.resolve(node, context);
        };
        let ports =
            config
                .ports
                .as_ref()
                .ok_or_else(|| NodeResolutionError::InvalidConfiguration {
                    reason: "multibus fixture requires a saved provider layout".into(),
                })?;
        Ok(ResolvedNode {
            inputs: compiled_buses(&ports.inputs, PortDirection::Input),
            outputs: compiled_buses(&ports.outputs, PortDirection::Output),
        })
    }
}

fn compiled_buses(buses: &[ProviderBus], direction: PortDirection) -> Vec<CompiledPort> {
    buses
        .iter()
        .map(|bus| CompiledPort::new(bus.id.clone(), direction, bus.channels))
        .collect()
}

#[derive(Default)]
struct Counts {
    input_channels: AtomicUsize,
    output_channels: AtomicUsize,
    process_calls: AtomicUsize,
}

struct MultibusProcessor(Arc<Counts>);

impl HostedAudioProcessor for MultibusProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        self.0.process_calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(input.len(), frames * 3);
        assert_eq!(output.len(), frames * 3);
        for frame in 0..frames {
            let offset = frame * 3;
            output[offset] = input[offset] + 100.0;
            output[offset + 1] = input[offset + 1] + 100.0;
            output[offset + 2] = input[offset + 2] + 200.0;
        }
        true
    }
}

struct MultibusFactory(Arc<Counts>);

impl HostedProcessorFactory for MultibusFactory {
    fn prepare(
        &self,
        request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        self.0
            .input_channels
            .store(usize::from(request.input_channels), Ordering::Relaxed);
        self.0
            .output_channels
            .store(usize::from(request.output_channels), Ordering::Relaxed);
        Ok(Box::new(MultibusProcessor(Arc::clone(&self.0))))
    }
}

struct FailedMultibusProcessor(Arc<Counts>);

impl HostedAudioProcessor for FailedMultibusProcessor {
    fn process(&mut self, _: &[f32], output: &mut [f32], _: usize) -> bool {
        self.0.process_calls.fetch_add(1, Ordering::Relaxed);
        output.fill(999.0);
        false
    }
}

struct FailedMultibusFactory(Arc<Counts>);

impl HostedProcessorFactory for FailedMultibusFactory {
    fn prepare(
        &self,
        _: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        Ok(Box::new(FailedMultibusProcessor(Arc::clone(&self.0))))
    }
}

fn source_node(id: &str, channels: u16) -> NodeSpec {
    node(
        id,
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::DefaultInput,
            output_channels: channels,
            gain_db: 0.0,
        }),
    )
}

fn sink_node(id: &str, channels: u16) -> NodeSpec {
    node(
        id,
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: channels,
        }),
    )
}

fn execution(layout: ProviderPorts, control: mixrelay_audio_graph::NodeControl) -> ExecutionPlan {
    GraphCompiler::new(MultibusResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![
                    source_node("a-main-source", 2),
                    source_node("b-side-source", 1),
                    provider_node(layout, control),
                    sink_node("y-aux-sink", 1),
                    sink_node("z-main-sink", 2),
                ],
                vec![
                    edge("main-in", "a-main-source", "audio", "provider", "audio"),
                    edge(
                        "side-in",
                        "b-side-source",
                        "audio",
                        "provider",
                        SIDECHAIN_PORT,
                    ),
                    edge("aux-out", "provider", SIDECHAIN_PORT, "y-aux-sink", "audio"),
                    edge("main-out", "provider", "audio", "z-main-sink", "audio"),
                ],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("multibus fixture graph should compile")
}

fn single_bus_execution(
    input_kind: ProviderBusKind,
    output_kind: ProviderBusKind,
) -> ExecutionPlan {
    let layout = ProviderPorts {
        layout_id: format!("single-{input_kind:?}-{output_kind:?}"),
        inputs: vec![bus("audio", input_kind, 2, ProviderChannelFormat::Stereo)],
        outputs: vec![bus("audio", output_kind, 2, ProviderChannelFormat::Stereo)],
    };
    GraphCompiler::new(MultibusResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![
                    source_node("source", 2),
                    provider_node(layout, mixrelay_audio_graph::NodeControl::default()),
                    sink_node("sink", 2),
                ],
                vec![
                    edge("provider-input", "source", "audio", "provider", "audio"),
                    edge("provider-output", "provider", "audio", "sink", "audio"),
                ],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("single-bus fixture graph should compile")
}

struct MismatchedSingleBusResolver;

impl NodeResolver for MismatchedSingleBusResolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        if !matches!(&node.config, NodeConfig::Provider(_)) {
            return BuiltInNodeResolver.resolve(node, context);
        }
        Ok(ResolvedNode {
            inputs: vec![CompiledPort::new(
                PortId::from("audio"),
                PortDirection::Input,
                1,
            )],
            outputs: vec![CompiledPort::new(
                PortId::from("audio"),
                PortDirection::Output,
                1,
            )],
        })
    }
}

fn provider_node(layout: ProviderPorts, control: mixrelay_audio_graph::NodeControl) -> NodeSpec {
    let mut provider = node(
        "provider",
        NodeConfig::Provider(ProviderNodeConfig {
            type_id: "fixture:multibus".into(),
            ports: Some(layout),
            parameters: serde_json::Value::Null,
        }),
    );
    provider.control = control;
    provider
}

fn provider_only_execution(layout: ProviderPorts) -> ExecutionPlan {
    GraphCompiler::new(MultibusResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![provider_node(
                    layout,
                    mixrelay_audio_graph::NodeControl::default(),
                )],
                vec![],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("provider node should compile before engine geometry validation")
}

fn fallback_execution() -> ExecutionPlan {
    let mut layout = provider_layout();
    layout.outputs = vec![
        bus(
            "audio",
            ProviderBusKind::Main,
            3,
            ProviderChannelFormat::Discrete,
        ),
        bus(
            SIDECHAIN_PORT,
            ProviderBusKind::Main,
            1,
            ProviderChannelFormat::Mono,
        ),
        bus(
            "bus:2",
            ProviderBusKind::Auxiliary,
            2,
            ProviderChannelFormat::Stereo,
        ),
    ];
    GraphCompiler::new(MultibusResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![
                    source_node("a-main-source", 2),
                    source_node("b-side-source", 1),
                    provider_node(layout, mixrelay_audio_graph::NodeControl::default()),
                    sink_node("x-aux-sink", 2),
                    sink_node("y-second-main-sink", 1),
                    sink_node("z-main-sink", 3),
                ],
                vec![
                    edge("main-in", "a-main-source", "audio", "provider", "audio"),
                    edge(
                        "side-in",
                        "b-side-source",
                        "audio",
                        "provider",
                        SIDECHAIN_PORT,
                    ),
                    edge("aux-out", "provider", "bus:2", "x-aux-sink", "audio"),
                    edge(
                        "second-main-out",
                        "provider",
                        SIDECHAIN_PORT,
                        "y-second-main-sink",
                        "audio",
                    ),
                    edge("main-out", "provider", "audio", "z-main-sink", "audio"),
                ],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("fallback fixture graph should compile")
}

#[test]
fn hosted_multibus_gathers_scatter_short_blocks_without_rt_heap_work_and_off_skips_dsp() {
    let counts = Arc::new(Counts::default());
    let execution = execution(
        provider_layout(),
        mixrelay_audio_graph::NodeControl::default(),
    );
    let mut processor = BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(crate::PeakMeterBank::new(16, 8)),
        &MultibusFactory(Arc::clone(&counts)),
    )
    .expect("valid multibus provider should prepare");
    assert_eq!(counts.input_channels.load(Ordering::Relaxed), 3);
    assert_eq!(counts.output_channels.load(Ordering::Relaxed), 3);

    let main_input = [1.0, 2.0, 3.0, 4.0];
    let sidechain_input = [10.0, 20.0];
    let sources: [&[f32]; 2] = [&main_input, &sidechain_input];
    let mut auxiliary_output = [0.0; 2];
    let mut main_output = [0.0; 4];
    let mut sinks: [&mut [f32]; 2] = [&mut auxiliary_output, &mut main_output];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_block(2, &sources, &mut sinks).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(
        main_output.map(f32::to_bits),
        [101.0, 102.0, 103.0, 104.0].map(f32::to_bits)
    );
    assert_eq!(
        auxiliary_output.map(f32::to_bits),
        [210.0, 220.0].map(f32::to_bits)
    );
    assert_eq!(counts.process_calls.load(Ordering::Relaxed), 1);

    let exchange = crate::ProcessorExchange::new(1);
    exchange
        .publish_controls(
            processor.plan().controls(),
            2,
            &[RuntimeControlUpdate {
                node_id: NodeId::from("provider"),
                control: mixrelay_audio_graph::NodeControl {
                    enabled: false,
                    muted: false,
                },
                gain_db: None,
            }],
        )
        .unwrap();
    auxiliary_output.fill(9.0);
    main_output.fill(9.0);
    let mut sinks: [&mut [f32]; 2] = [&mut auxiliary_output, &mut main_output];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_block(2, &sources, &mut sinks).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(main_output.map(f32::to_bits), [0.0; 4].map(f32::to_bits));
    assert_eq!(
        auxiliary_output.map(f32::to_bits),
        [0.0; 2].map(f32::to_bits)
    );
    assert_eq!(counts.process_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn multibus_failure_fallback_copies_only_matched_main_buses() {
    let counts = Arc::new(Counts::default());
    let graph = fallback_execution();
    let mut processor = BlockProcessor::prepare_with_hosted_processors(
        &graph,
        Arc::new(crate::PeakMeterBank::new(16, 8)),
        &FailedMultibusFactory(Arc::clone(&counts)),
    )
    .expect("valid fallback fixture should prepare");
    let main_input = [1.0, 2.0, 3.0, 4.0];
    let sidechain_input = [10.0, 20.0];
    let sources: [&[f32]; 2] = [&main_input, &sidechain_input];
    let mut auxiliary_output = [9.0; 4];
    let mut unmatched_main_output = [9.0; 2];
    let mut main_output = [9.0; 6];
    let mut sinks: [&mut [f32]; 3] = [
        &mut auxiliary_output,
        &mut unmatched_main_output,
        &mut main_output,
    ];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_block(2, &sources, &mut sinks).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(
        main_output.map(f32::to_bits),
        [1.0, 2.0, 0.0, 3.0, 4.0, 0.0].map(f32::to_bits)
    );
    assert_eq!(
        unmatched_main_output.map(f32::to_bits),
        [0.0; 2].map(f32::to_bits)
    );
    assert_eq!(
        auxiliary_output.map(f32::to_bits),
        [0.0; 4].map(f32::to_bits)
    );
    assert_eq!(counts.process_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn saved_non_main_single_bus_false_return_does_not_leak_signal() {
    for (input_kind, output_kind) in [
        (ProviderBusKind::Sidechain, ProviderBusKind::Main),
        (ProviderBusKind::Main, ProviderBusKind::Auxiliary),
        (ProviderBusKind::Sidechain, ProviderBusKind::Auxiliary),
    ] {
        let counts = Arc::new(Counts::default());
        let graph = single_bus_execution(input_kind, output_kind);
        let mut processor = BlockProcessor::prepare_with_hosted_processors(
            &graph,
            Arc::new(crate::PeakMeterBank::new(16, 8)),
            &FailedMultibusFactory(Arc::clone(&counts)),
        )
        .expect("valid one-bus layout should prepare");
        let input = [1.0, 2.0, 3.0, 4.0];
        let sources: [&[f32]; 1] = [&input];
        let mut output = [9.0; 4];
        let mut sinks: [&mut [f32]; 1] = [&mut output];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor.process_block(2, &sources, &mut sinks).unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert_eq!(output.map(f32::to_bits), [0.0; 4].map(f32::to_bits));
        assert_eq!(counts.process_calls.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn saved_main_single_bus_keeps_direct_fallback_path() {
    let counts = Arc::new(Counts::default());
    let graph = single_bus_execution(ProviderBusKind::Main, ProviderBusKind::Main);
    let mut processor = BlockProcessor::prepare_with_hosted_processors(
        &graph,
        Arc::new(crate::PeakMeterBank::new(16, 8)),
        &FailedMultibusFactory(Arc::clone(&counts)),
    )
    .expect("valid main layout should prepare");
    assert_eq!(processor.hosted.scratch_samples(), 0);
    let input = [1.0, 2.0, 3.0, 4.0];
    let sources: [&[f32]; 1] = [&input];
    let mut output = [9.0; 4];
    let mut sinks: [&mut [f32]; 1] = [&mut output];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_block(2, &sources, &mut sinks).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
    assert_eq!(counts.process_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn saved_single_bus_layout_must_match_compiled_port_geometry() {
    let graph = GraphCompiler::new(MismatchedSingleBusResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![provider_node(
                    ProviderPorts::canonical_stereo("saved-stereo"),
                    mixrelay_audio_graph::NodeControl::default(),
                )],
                vec![],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("fixture resolver may expose a stale compiled snapshot");
    let result = BlockProcessor::prepare_with_hosted_processors(
        &graph,
        Arc::new(crate::PeakMeterBank::new(16, 8)),
        &MultibusFactory(Arc::new(Counts::default())),
    );
    assert!(matches!(
        result,
        Err(RenderPlanError::ProviderLayoutMismatch { .. })
    ));
}

#[test]
fn hosted_provider_rejects_zero_per_bus_and_oversized_aggregate_geometry() {
    let invalid_layouts = [
        ProviderPorts {
            layout_id: "zero-channel".into(),
            inputs: vec![bus(
                "audio",
                ProviderBusKind::Main,
                0,
                ProviderChannelFormat::Discrete,
            )],
            outputs: vec![bus(
                "audio",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            )],
        },
        ProviderPorts {
            layout_id: "oversized-bus".into(),
            inputs: vec![bus(
                "audio",
                ProviderBusKind::Main,
                65,
                ProviderChannelFormat::Discrete,
            )],
            outputs: vec![bus(
                "audio",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            )],
        },
        ProviderPorts {
            layout_id: "oversized-total".into(),
            inputs: vec![
                bus(
                    "audio",
                    ProviderBusKind::Main,
                    40,
                    ProviderChannelFormat::Discrete,
                ),
                bus(
                    "bus:1",
                    ProviderBusKind::Sidechain,
                    25,
                    ProviderChannelFormat::Discrete,
                ),
            ],
            outputs: vec![bus(
                "audio",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            )],
        },
    ];
    for layout in invalid_layouts {
        let graph = provider_only_execution(layout);
        let result = BlockProcessor::prepare_with_hosted_processors(
            &graph,
            Arc::new(crate::PeakMeterBank::new(16, 8)),
            &MultibusFactory(Arc::new(Counts::default())),
        );
        assert!(matches!(
            result,
            Err(RenderPlanError::InvalidProviderBusChannels { .. }
                | RenderPlanError::InvalidProviderTotalChannels { .. })
        ));
    }
}
