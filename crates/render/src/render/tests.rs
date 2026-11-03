//! Shared graph fixtures and heap accounting for render-contract tests.
mod builtin_effects;
mod compensation;
mod composition;
mod contracts;
mod controls;
mod cycle_admission;
mod device_bus;
mod dynamics_telemetry;
mod history_policy;
mod memory;
mod memory_pressure;
mod nonfinite;
mod provider_buses;
mod recorder;
mod rendering;
mod scaling;
mod virtual_routes;
use super::*;
use mixrelay_audio_graph::{AUDIO_PORT_ID, FROM_OS_PORT_ID, TO_OS_PORT_ID};
use mixrelay_audio_graph::{
    ApplicationCaptureRequest, ApplicationSelector, AudioDeviceInputConfig,
    AudioDeviceOutputConfig, BuiltInNodeResolver, ChannelLayout, ChannelMapping, ChannelRoute,
    CompileContext, CompileLimits, CompiledPort, DeviceSelector, EdgeId, EdgeSpec, GainConfig,
    GraphCompiler, GraphDocument, MixerConfig, NodeResolutionError, NodeResolver, NodeSpec,
    PortDirection, ProviderNodeConfig, ResolvedNode, SourcePlaybackPolicy, StreamSpec,
    VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_audio_io::process::{ProcessCycle, ProcessReport, TransferOutcome};

const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: 4,
};

// Thread-local instrumentation keeps parallel tests out of the measured
// callback window. All native/plugin preparation stays outside that window.
std::thread_local! {
    static HEAP_OPERATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static HEAP_BALANCE: std::cell::Cell<Option<HeapBalance>> = const { std::cell::Cell::new(None) };
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HeapBalance {
    allocated_bytes: usize,
    freed_bytes: usize,
    allocations: usize,
    frees: usize,
}
fn account_heap_bytes(allocated: Option<usize>, freed: Option<usize>) {
    let _ = HEAP_BALANCE.try_with(|count| {
        if let Some(mut balance) = count.get() {
            if let Some(bytes) = allocated {
                balance.allocated_bytes += bytes;
                balance.allocations += 1;
            }
            if let Some(bytes) = freed {
                balance.freed_bytes += bytes;
                balance.frees += 1;
            }
            count.set(Some(balance));
        }
    });
}
struct CountedAllocator;
fn count_heap_operation() {
    let _ = HEAP_OPERATIONS.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}
// SAFETY: ownership, alignment and sizes are delegated unchanged to System.
unsafe impl std::alloc::GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        count_heap_operation();
        let pointer = unsafe { std::alloc::System.alloc(layout) };
        if !pointer.is_null() {
            account_heap_bytes(Some(layout.size()), None);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        count_heap_operation();
        account_heap_bytes(None, Some(layout.size()));
        unsafe { std::alloc::System.dealloc(pointer, layout) };
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        count_heap_operation();
        let pointer = unsafe { std::alloc::System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            account_heap_bytes(Some(layout.size()), None);
        }
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        count_heap_operation();
        let replacement = unsafe { std::alloc::System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            account_heap_bytes(Some(size), Some(layout.size()));
        }
        replacement
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

struct CallbackIo {
    read_frames: u32,
    discontinuity: bool,
    activity: mixrelay_audio_io::process::InputActivity,
    write_frames: u32,
    value: f32,
    output: [f32; 8],
    reads: usize,
    writes: usize,
    observed: Option<ProcessCycle>,
}

impl ExternalIo for CallbackIo {
    fn read_source(
        &mut self,
        _: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.reads += 1;
        self.observed = Some(*cycle);
        destination.fill(self.value);
        TransferOutcome {
            activity: self.activity,
            discontinuity: self.discontinuity,
            ..TransferOutcome::complete(self.read_frames)
        }
    }
    fn write_sink(
        &mut self,
        _: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.writes += 1;
        assert_eq!(self.observed, Some(*cycle));
        self.output[..source.len()].copy_from_slice(source);
        TransferOutcome::complete(self.write_frames)
    }
}

fn callback_fixture() -> (BlockProcessor, CallbackIo, ProcessCycle) {
    let execution = compile(
        vec![virtual_endpoint("io")],
        vec![edge("loop", "io", FROM_OS_PORT_ID, "io", TO_OS_PORT_ID)],
    );
    let processor =
        BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap();
    let io = CallbackIo {
        read_frames: 4,
        discontinuity: false,
        activity: mixrelay_audio_io::process::InputActivity::Active,
        write_frames: 4,
        value: 0.0,
        output: [99.0; 8],
        reads: 0,
        writes: 0,
        observed: None,
    };
    let cycle = ProcessCycle {
        clock: mixrelay_audio_io::process::ClockDomain {
            authority: mixrelay_audio_io::session::ClockAuthority::Device,
            id: 1,
            epoch: 1,
        },
        frames: 4,
        sample_rate: 48_000,
        first_frame: 0,
        monotonic_ns: Some(123),
        discontinuity: false,
    };
    (processor, io, cycle)
}

fn compile(nodes: Vec<NodeSpec>, edges: Vec<EdgeSpec>) -> ExecutionPlan {
    GraphCompiler::default()
        .compile(
            &GraphDocument::new(1, nodes, edges),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("test graph should compile")
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: NodeId::from(id),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config,
    }
}

fn endpoint(node_id: &str, port_id: &str) -> AudioEndpoint {
    AudioEndpoint {
        node_id: NodeId::from(node_id),
        port_id: PortId::from(port_id),
    }
}

fn edge(
    id: &str,
    source_node: &str,
    source_port: &str,
    destination_node: &str,
    destination_port: &str,
) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: endpoint(source_node, source_port),
        destination: endpoint(destination_node, destination_port),
        channel_mapping: ChannelMapping::Auto,
    }
}

fn stereo_stream() -> StreamSpec {
    StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    }
}

fn app_source(id: &str) -> NodeSpec {
    node(
        id,
        NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
            selector: ApplicationSelector::BundleId {
                bundle_id: format!("io.mixrelay.{id}"),
                include_descendants: true,
            },
            stream: stereo_stream(),
            source_playback: SourcePlaybackPolicy::Preserve,
            forwarding_endpoint: None,
            gain_db: 0.0,
            requires_reselection: false,
        }),
    )
}

fn device_input(id: &str) -> NodeSpec {
    node(
        id,
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::DefaultInput,
            output_channels: 2,
            gain_db: 0.0,
        }),
    )
}

fn device_output(id: &str) -> NodeSpec {
    node(
        id,
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: 2,
        }),
    )
}

fn virtual_endpoint(id: &str) -> NodeSpec {
    node(
        id,
        NodeConfig::VirtualEndpointTransport(VirtualEndpointSpec {
            id: VirtualEndpointId::from(id),
            display_name: id.into(),
            render: Some(stereo_stream()),
            capture: Some(stereo_stream()),
            supported_sample_rates: vec![48_000],
            default_sample_rate: 48_000,
        }),
    )
}

fn device_bus_processor(channels: u16) -> BlockProcessor {
    let execution = compile(
        vec![
            node(
                "in",
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: channels,
                    gain_db: 0.0,
                }),
            ),
            node("bus", NodeConfig::DeviceBus { channels }),
            node(
                "out",
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::DefaultOutput,
                    input_channels: channels,
                }),
            ),
        ],
        vec![
            edge("publish", "in", AUDIO_PORT_ID, "bus", AUDIO_PORT_ID),
            edge("read", "bus", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap()
}

struct IndependentBusIo {
    source_a: [f32; 8],
    source_b: [f32; 8],
    output_a: [f32; 8],
    output_b: [f32; 8],
    source_a_discontinuity: bool,
    admission: CycleAdmission,
}

impl ExternalIo for IndependentBusIo {
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
        let (source, discontinuity) = match binding.node_id().as_str() {
            "source-a" => (&self.source_a, self.source_a_discontinuity),
            "source-b" => (&self.source_b, false),
            other => panic!("unexpected test source binding {other}"),
        };
        destination.copy_from_slice(&source[..destination.len()]);
        TransferOutcome {
            discontinuity,
            ..TransferOutcome::complete(cycle.frames)
        }
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        _cycle: &ProcessCycle,
    ) -> TransferOutcome {
        let destination = match binding.node_id().as_str() {
            "sink-a" => &mut self.output_a,
            "sink-b" => &mut self.output_b,
            other => panic!("unexpected test sink binding {other}"),
        };
        destination[..source.len()].copy_from_slice(source);
        TransferOutcome::complete(u32::try_from(source.len() / 2).unwrap())
    }
}

fn independent_bus_fixture() -> (BlockProcessor, IndependentBusIo, ProcessCycle) {
    let execution = compile(
        vec![
            device_input("source-a"),
            node("bus-a", NodeConfig::DeviceBus { channels: 2 }),
            device_output("sink-a"),
            device_input("source-b"),
            node("bus-b", NodeConfig::DeviceBus { channels: 2 }),
            device_output("sink-b"),
        ],
        vec![
            edge(
                "publish-a",
                "source-a",
                AUDIO_PORT_ID,
                "bus-a",
                AUDIO_PORT_ID,
            ),
            edge("read-a", "bus-a", AUDIO_PORT_ID, "sink-a", AUDIO_PORT_ID),
            edge(
                "publish-b",
                "source-b",
                AUDIO_PORT_ID,
                "bus-b",
                AUDIO_PORT_ID,
            ),
            edge("read-b", "bus-b", AUDIO_PORT_ID, "sink-b", AUDIO_PORT_ID),
        ],
    );
    (
        BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap(),
        IndependentBusIo {
            source_a: [1.0; 8],
            source_b: [2.0; 8],
            output_a: [0.0; 8],
            output_b: [0.0; 8],
            source_a_discontinuity: false,
            admission: CycleAdmission::Process,
        },
        ProcessCycle {
            clock: mixrelay_audio_io::process::ClockDomain {
                authority: mixrelay_audio_io::session::ClockAuthority::Device,
                id: 1,
                epoch: 0,
            },
            sample_rate: FORMAT.sample_rate,
            frames: FORMAT.block_frames,
            first_frame: 0,
            monotonic_ns: None,
            discontinuity: false,
        },
    )
}

fn processor_graph() -> ExecutionPlan {
    let nodes = vec![
        app_source("app"),
        device_input("mic"),
        node(
            "mix",
            NodeConfig::Mixer(MixerConfig {
                inputs: vec![PortId::from("app"), PortId::from("mic")],
                channels: 2,
            }),
        ),
        node(
            "gain",
            NodeConfig::Gain(GainConfig {
                channels: 2,
                gain_db: -6.020_600_3,
            }),
        ),
        virtual_endpoint("virtual"),
        device_output("monitor"),
    ];
    let mut swapped = edge(
        "virtual-monitor",
        "virtual",
        FROM_OS_PORT_ID,
        "monitor",
        AUDIO_PORT_ID,
    );
    swapped.channel_mapping = ChannelMapping::Explicit {
        routes: vec![
            ChannelRoute {
                source_channel: 0,
                destination_channel: 1,
            },
            ChannelRoute {
                source_channel: 1,
                destination_channel: 0,
            },
        ],
    };
    compile(
        nodes,
        vec![
            edge("app-mix", "app", AUDIO_PORT_ID, "mix", "app"),
            edge("mic-mix", "mic", AUDIO_PORT_ID, "mix", "mic"),
            edge("mix-gain", "mix", AUDIO_PORT_ID, "gain", AUDIO_PORT_ID),
            edge(
                "gain-virtual",
                "gain",
                AUDIO_PORT_ID,
                "virtual",
                TO_OS_PORT_ID,
            ),
            swapped,
        ],
    )
}

struct TestProviderResolver;

impl NodeResolver for TestProviderResolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        if matches!(node.config, NodeConfig::Provider(_)) {
            return Ok(ResolvedNode {
                inputs: vec![CompiledPort::new(
                    PortId::from(AUDIO_PORT_ID),
                    PortDirection::Input,
                    2,
                )],
                outputs: vec![CompiledPort::new(
                    PortId::from(AUDIO_PORT_ID),
                    PortDirection::Output,
                    2,
                )],
            });
        }
        BuiltInNodeResolver.resolve(node, context)
    }
}

struct DoubleProcessor;

impl HostedAudioProcessor for DoubleProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        let samples = frames * 2;
        for index in 0..samples {
            output[index] = input[index] * 2.0;
        }
        true
    }
}

struct TestProviderFactory;

impl HostedProcessorFactory for TestProviderFactory {
    fn prepare(
        &self,
        _request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        Ok(Box::new(DoubleProcessor))
    }
}

#[derive(Clone, Copy)]
enum ResolverShape {
    MissingSourceOutput,
    MissingSinkInput,
    MissingMixerInput,
    MissingMixerOutput,
    MissingGainInput,
    MissingVirtualRender,
    MissingVirtualCapture,
    MissingGainOutput,
    DuplicateProviderPort,
    ValidProvider,
}

struct ShapeResolver(ResolverShape);

impl NodeResolver for ShapeResolver {
    fn resolve(
        &self,
        _node: &NodeSpec,
        _context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        let input = CompiledPort::new(PortId::from(AUDIO_PORT_ID), PortDirection::Input, 2);
        let output = CompiledPort::new(PortId::from(AUDIO_PORT_ID), PortDirection::Output, 2);
        Ok(match self.0 {
            ResolverShape::MissingSourceOutput | ResolverShape::MissingSinkInput => ResolvedNode {
                inputs: vec![],
                outputs: vec![],
            },
            ResolverShape::MissingMixerInput | ResolverShape::MissingGainInput => ResolvedNode {
                inputs: vec![],
                outputs: vec![output],
            },
            ResolverShape::MissingMixerOutput => ResolvedNode {
                inputs: vec![CompiledPort::new(
                    PortId::from("one"),
                    PortDirection::Input,
                    2,
                )],
                outputs: vec![],
            },
            ResolverShape::MissingVirtualRender => ResolvedNode {
                inputs: vec![CompiledPort::new(
                    PortId::from(TO_OS_PORT_ID),
                    PortDirection::Input,
                    2,
                )],
                outputs: vec![],
            },
            ResolverShape::MissingVirtualCapture => ResolvedNode {
                inputs: vec![],
                outputs: vec![CompiledPort::new(
                    PortId::from(FROM_OS_PORT_ID),
                    PortDirection::Output,
                    2,
                )],
            },
            ResolverShape::MissingGainOutput => ResolvedNode {
                inputs: vec![input],
                outputs: vec![],
            },
            ResolverShape::DuplicateProviderPort => ResolvedNode {
                inputs: vec![input.clone(), input],
                outputs: vec![output],
            },
            ResolverShape::ValidProvider => ResolvedNode {
                inputs: vec![input],
                outputs: vec![output],
            },
        })
    }
}

fn custom_plan(config: NodeConfig, shape: ResolverShape) -> ExecutionPlan {
    GraphCompiler::new(ShapeResolver(shape))
        .compile(
            &GraphDocument::new(1, vec![node("custom", config)], vec![]),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("custom graph should compile")
}
