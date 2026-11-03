use super::*;
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, BuiltInNodeResolver, CompileContext,
    CompileLimits, DeviceSelector, MixerConfig, NodeControl, ProviderPorts,
};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const TEST_DELAY: u32 = 5;
const UNKNOWN_LATENCY: u32 = u32::MAX;
const PARTITIONS: [usize; 3] = [1, 32, 128];

#[derive(Default)]
struct ProcessorCounts {
    prepares: AtomicUsize,
    calls: AtomicUsize,
    suspensions: AtomicUsize,
    active_changes: AtomicUsize,
}

struct LatencyState {
    actual_frames: AtomicU32,
    reported_frames: AtomicU32,
    counts: Arc<ProcessorCounts>,
}

struct DelayFactory {
    state: Arc<LatencyState>,
    maximum_delay_frames: usize,
}

struct DelayProcessor {
    identity: HostedProcessorIdentity,
    state: Arc<LatencyState>,
    channels: usize,
    maximum_delay_frames: usize,
    history: Vec<f32>,
    delay_frames: usize,
    cursor_frame: usize,
    valid_frames: usize,
}

struct LatencyReader(Arc<LatencyState>);

impl DelayFactory {
    fn new(actual_frames: u32, reported_frames: Option<u32>) -> Self {
        Self {
            state: Arc::new(LatencyState {
                actual_frames: AtomicU32::new(actual_frames),
                reported_frames: AtomicU32::new(reported_frames.unwrap_or(UNKNOWN_LATENCY)),
                counts: Arc::new(ProcessorCounts::default()),
            }),
            maximum_delay_frames: 64,
        }
    }

    fn counts(&self) -> &Arc<ProcessorCounts> {
        &self.state.counts
    }
}

impl HostedLatencySource for LatencyReader {
    fn frames(&self) -> Option<u32> {
        match self.0.reported_frames.load(Ordering::Acquire) {
            UNKNOWN_LATENCY => None,
            frames => Some(frames),
        }
    }
}

impl HostedAudioProcessor for DelayProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        self.state.counts.calls.fetch_add(1, Ordering::Relaxed);
        let Some(sample_count) = frames.checked_mul(self.channels) else {
            return false;
        };
        if input.len() < sample_count || output.len() < sample_count {
            return false;
        }

        let Ok(delay_frames) = usize::try_from(self.state.actual_frames.load(Ordering::Acquire))
        else {
            return false;
        };
        if delay_frames > self.maximum_delay_frames {
            return false;
        }
        if delay_frames != self.delay_frames {
            self.delay_frames = delay_frames;
            self.cursor_frame = 0;
            self.valid_frames = 0;
        }
        if delay_frames == 0 {
            output[..sample_count].copy_from_slice(&input[..sample_count]);
            return true;
        }

        let mut completed_frames = 0;
        while completed_frames < frames {
            let ring_remaining = delay_frames - self.cursor_frame;
            let chunk_frames = (frames - completed_frames).min(ring_remaining);
            let chunk_samples = chunk_frames * self.channels;
            let history_start = self.cursor_frame * self.channels;
            let pcm_start = completed_frames * self.channels;
            let warmup_frames = chunk_frames.min(delay_frames - self.valid_frames);
            let warmup_samples = warmup_frames * self.channels;
            if warmup_samples > 0 {
                let history_end = history_start + warmup_samples;
                let pcm_end = pcm_start + warmup_samples;
                self.history[history_start..history_end]
                    .copy_from_slice(&input[pcm_start..pcm_end]);
                output[pcm_start..pcm_end].fill(0.0);
                self.valid_frames += warmup_frames;
            }

            let history_hot_start = history_start + warmup_samples;
            let pcm_hot_start = pcm_start + warmup_samples;
            let hot_samples = chunk_samples - warmup_samples;
            if hot_samples > 0 {
                let history_hot_end = history_hot_start + hot_samples;
                let pcm_hot_end = pcm_hot_start + hot_samples;
                output[pcm_hot_start..pcm_hot_end]
                    .copy_from_slice(&self.history[history_hot_start..history_hot_end]);
                self.history[history_hot_start..history_hot_end]
                    .copy_from_slice(&input[pcm_hot_start..pcm_hot_end]);
            }

            self.cursor_frame += chunk_frames;
            if self.cursor_frame == delay_frames {
                self.cursor_frame = 0;
            }
            completed_frames += chunk_frames;
        }
        true
    }

    fn suspend_processing(&mut self) {
        self.state
            .counts
            .suspensions
            .fetch_add(1, Ordering::Relaxed);
        self.cursor_frame = 0;
        self.valid_frames = 0;
    }

    fn reuse_identity(&self) -> Option<&HostedProcessorIdentity> {
        Some(&self.identity)
    }

    fn set_active(&mut self, _active: bool) {
        self.state
            .counts
            .active_changes
            .fetch_add(1, Ordering::Relaxed);
    }

    fn latency_source(&self) -> Option<Arc<dyn HostedLatencySource>> {
        Some(Arc::new(LatencyReader(Arc::clone(&self.state))))
    }
}

impl HostedProcessorFactory for DelayFactory {
    fn prepare(
        &self,
        request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        self.state.counts.prepares.fetch_add(1, Ordering::Relaxed);
        let channels = usize::from(request.input_channels);
        Ok(Box::new(DelayProcessor {
            identity: HostedProcessorIdentity::new(),
            state: Arc::clone(&self.state),
            channels,
            maximum_delay_frames: self.maximum_delay_frames,
            history: vec![0.0; self.maximum_delay_frames * channels],
            delay_frames: usize::MAX,
            cursor_frame: 0,
            valid_frames: 0,
        }))
    }
}

struct DelayProviderResolver;

impl NodeResolver for DelayProviderResolver {
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
                    reason: "delay fixture requires provider ports".into(),
                })?;
        ports
            .validate()
            .map_err(|error| NodeResolutionError::InvalidConfiguration {
                reason: error.to_string(),
            })?;
        Ok(ResolvedNode {
            inputs: ports
                .inputs
                .iter()
                .map(|bus| CompiledPort::new(bus.id.clone(), PortDirection::Input, bus.channels))
                .collect(),
            outputs: ports
                .outputs
                .iter()
                .map(|bus| CompiledPort::new(bus.id.clone(), PortDirection::Output, bus.channels))
                .collect(),
        })
    }
}

fn device_input_with_channels(channels: u16) -> NodeSpec {
    node(
        "in",
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::DefaultInput,
            output_channels: channels,
            gain_db: 0.0,
        }),
    )
}

fn device_output_with_channels(channels: u16) -> NodeSpec {
    node(
        "out",
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: channels,
        }),
    )
}

fn provider_node(channels: u16, control: NodeControl) -> NodeSpec {
    let mut provider = node(
        "effect",
        NodeConfig::Provider(ProviderNodeConfig {
            type_id: mixrelay_audio_graph::NodeTypeId::from("test:latency-delay"),
            parameters: serde_json::json!({}),
            ports: Some(ProviderPorts::single_bus(
                "delay-layout",
                channels,
                channels,
            )),
        }),
    );
    provider.control = control;
    provider
}

fn diamond_execution(
    revision: u64,
    channels: u16,
    block_frames: u32,
    add_direct_gain: bool,
    controls: [NodeControl; 2],
) -> ExecutionPlan {
    let [provider_control, mixer_control] = controls;
    let mut mixer = node(
        "mix",
        NodeConfig::Mixer(MixerConfig {
            inputs: vec![PortId::from("dry"), PortId::from("wet")],
            channels,
        }),
    );
    mixer.control = mixer_control;
    let mut nodes = vec![
        device_input_with_channels(channels),
        provider_node(channels, provider_control),
        mixer,
        device_output_with_channels(channels),
    ];
    let mut edges = vec![
        edge("input-effect", "in", AUDIO_PORT_ID, "effect", AUDIO_PORT_ID),
        edge("effect-mix", "effect", AUDIO_PORT_ID, "mix", "wet"),
        edge("mix-output", "mix", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
    ];
    if add_direct_gain {
        nodes.push(node(
            "direct-gain",
            NodeConfig::Gain(GainConfig {
                channels,
                gain_db: 0.0,
            }),
        ));
        edges.push(edge(
            "input-direct-gain",
            "in",
            AUDIO_PORT_ID,
            "direct-gain",
            AUDIO_PORT_ID,
        ));
        edges.push(edge(
            "direct-gain-mix",
            "direct-gain",
            AUDIO_PORT_ID,
            "mix",
            "dry",
        ));
    } else {
        edges.push(edge("input-mix", "in", AUDIO_PORT_ID, "mix", "dry"));
    }

    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames,
    };
    GraphCompiler::new(DelayProviderResolver)
        .compile(
            &GraphDocument::new(revision, nodes, edges),
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .expect("delay compensation graph should compile")
}

fn linear_execution(
    revision: u64,
    channels: u16,
    block_frames: u32,
    provider_control: NodeControl,
) -> ExecutionPlan {
    GraphCompiler::new(DelayProviderResolver)
        .compile(
            &GraphDocument::new(
                revision,
                vec![
                    device_input_with_channels(channels),
                    provider_node(channels, provider_control),
                    device_output_with_channels(channels),
                ],
                vec![
                    edge("input-effect", "in", AUDIO_PORT_ID, "effect", AUDIO_PORT_ID),
                    edge(
                        "effect-output",
                        "effect",
                        AUDIO_PORT_ID,
                        "out",
                        AUDIO_PORT_ID,
                    ),
                ],
            ),
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames,
                },
                limits: CompileLimits::default(),
            },
        )
        .expect("hosted delay graph should compile")
}

fn prepare_processor(execution: &ExecutionPlan, factory: &DelayFactory) -> BlockProcessor {
    BlockProcessor::prepare_with_hosted_processors(
        execution,
        Arc::new(crate::PeakMeterBank::new(8, 64)),
        factory,
    )
    .expect("hosted delay processor should prepare")
}

fn render_without_heap(
    processor: &mut BlockProcessor,
    frames: usize,
    input: &[f32],
    output: &mut [f32],
) {
    let frames = u32::try_from(frames).expect("test callback frame count fits u32");
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    let result = processor.process_block(frames, &[input], &mut [output]);
    let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
    assert_eq!(operations, Some(0));
    result.expect("prepared graph should process without callback heap operations");
}

fn render_impulse_stream(
    processor: &mut BlockProcessor,
    channels: usize,
    impulse_frame: usize,
    impulse_scale: f32,
) -> Vec<f32> {
    let total_frames: usize = PARTITIONS.iter().sum();
    let mut input = vec![0.0; total_frames * channels];
    for channel in 0..channels {
        let channel_value = f32::from(u16::try_from(channel + 1).expect("test width fits u16"));
        input[impulse_frame * channels + channel] = channel_value * impulse_scale;
    }
    let mut output = vec![0.0; input.len()];
    let mut frame_offset = 0;
    for callback_frames in PARTITIONS {
        let sample_offset = frame_offset * channels;
        let sample_count = callback_frames * channels;
        render_without_heap(
            processor,
            callback_frames,
            &input[sample_offset..sample_offset + sample_count],
            &mut output[sample_offset..sample_offset + sample_count],
        );
        frame_offset += callback_frames;
    }
    output
}

fn assert_aligned_impulse(output: &[f32], channels: usize, delay_frames: usize, scale: f32) {
    let impulse_start = delay_frames * channels;
    for (index, sample) in output.iter().enumerate() {
        let expected = if (impulse_start..impulse_start + channels).contains(&index) {
            let channel = index - impulse_start;
            let value = f32::from(u16::try_from(channel + 1).expect("test width fits u16"));
            value * scale
        } else {
            0.0
        };
        assert_eq!(sample.to_bits(), expected.to_bits(), "sample {index}");
    }
}

fn publish_control(
    exchange: &crate::ProcessorExchange,
    plan: &RenderPlan,
    revision: u64,
    node_id: &str,
    control: NodeControl,
) {
    exchange
        .publish_controls(
            plan.controls(),
            revision,
            &[crate::RuntimeControlUpdate {
                node_id: NodeId::from(node_id),
                control,
                gain_db: None,
            }],
        )
        .expect("control publication should match the prepared graph");
}

#[test]
fn dry_and_wet_impulses_align_across_variable_partitions_and_channel_widths() {
    for channels in [2_u16, 32] {
        let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
        let execution = diamond_execution(
            1,
            channels,
            128,
            false,
            [NodeControl::default(), NodeControl::default()],
        );
        let mut processor = prepare_processor(&execution, &factory);
        let output = render_impulse_stream(&mut processor, usize::from(channels), 0, 1.0);

        assert_eq!(processor.compensation_state(), CompensationState::Current);
        assert_aligned_impulse(&output, usize::from(channels), TEST_DELAY as usize, 2.0);
    }
}

#[test]
fn unknown_host_latency_does_not_claim_or_apply_false_compensation() {
    let factory = DelayFactory::new(TEST_DELAY, None);
    let execution = diamond_execution(
        1,
        2,
        128,
        false,
        [NodeControl::default(), NodeControl::default()],
    );
    let mut processor = prepare_processor(&execution, &factory);
    let output = render_impulse_stream(&mut processor, 2, 0, 1.0);

    assert_eq!(processor.compensation_state(), CompensationState::Unknown);
    for frame in 0..PARTITIONS.iter().sum::<usize>() {
        for channel in 0..2 {
            let expected = if frame == 0 || frame == TEST_DELAY as usize {
                f32::from(u16::try_from(channel + 1).expect("test width fits u16"))
            } else {
                0.0
            };
            assert_eq!(output[frame * 2 + channel].to_bits(), expected.to_bits());
        }
    }
}

#[test]
fn bank_refresh_preserves_warmed_history_and_changes_latency_without_reprepare() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(
        1,
        2,
        128,
        false,
        [NodeControl::default(), NodeControl::default()],
    );
    let mut processor = Box::new(prepare_processor(&execution, &factory));
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);
    exchange.observe_plan(Arc::clone(&plan));

    let mut first_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[1.0, 1.0], &mut first_output);
    assert_eq!(
        first_output.map(f32::to_bits),
        [0.0_f32; 2].map(f32::to_bits)
    );
    assert!(exchange.refresh_latency().expect("initial bank refresh"));

    let revisions = exchange.plan_revisions();
    let mut native_activations = 0;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    let activated = exchange
        .activate_pending_configured(&mut processor, |_| {
            native_activations += 1;
            Ok::<(), String>(())
        })
        .expect("bank-only activation should not run native setup");
    let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
    assert_eq!(operations, Some(0));
    assert!(!activated);
    assert_eq!(native_activations, 0);
    assert_eq!(exchange.plan_revisions(), revisions);
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(factory.counts().prepares.load(Ordering::Relaxed), 1);

    let mut tail_output = [0.0; 8];
    render_without_heap(&mut processor, 4, &[0.0; 8], &mut tail_output);
    assert_eq!(
        tail_output.map(f32::to_bits),
        [0.0_f32; 8].map(f32::to_bits)
    );
    let mut impulse_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[0.0; 2], &mut impulse_output);
    assert_eq!(
        impulse_output.map(f32::to_bits),
        [2.0_f32; 2].map(f32::to_bits)
    );

    factory.state.actual_frames.store(7, Ordering::Release);
    factory.state.reported_frames.store(7, Ordering::Release);
    let mut pending_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[0.0; 2], &mut pending_output);
    assert_eq!(processor.compensation_state(), CompensationState::Pending);
    assert!(exchange.refresh_latency().expect("changed latency refresh"));
    assert!(!exchange.activate_pending(&mut processor));
    exchange.reclaim_retired();

    let updated = render_impulse_stream(&mut processor, 2, 0, 1.0);
    assert_eq!(processor.compensation_state(), CompensationState::Current);
    assert_aligned_impulse(&updated, 2, 7, 2.0);
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(exchange.plan_revisions(), revisions);
    assert_eq!(factory.counts().prepares.load(Ordering::Relaxed), 1);
    assert_eq!(factory.counts().active_changes.load(Ordering::Relaxed), 0);
    assert!(factory.counts().calls.load(Ordering::Relaxed) > 1);
}

#[test]
fn stale_bank_for_replaced_topology_cannot_apply_to_the_new_plan() {
    let factory = DelayFactory::new(3, Some(3));
    let old_execution = diamond_execution(1, 2, 128, false, [NodeControl::default(); 2]);
    let mut current = Box::new(prepare_processor(&old_execution, &factory));
    let exchange = crate::ProcessorExchange::new(1);
    exchange.observe_plan(Arc::clone(current.plan()));

    factory.state.actual_frames.store(7, Ordering::Release);
    factory.state.reported_frames.store(7, Ordering::Release);
    assert!(
        exchange
            .refresh_latency()
            .expect("old topology bank refresh")
    );

    factory.state.actual_frames.store(2, Ordering::Release);
    factory.state.reported_frames.store(2, Ordering::Release);
    let new_execution = diamond_execution(
        2,
        2,
        128,
        true,
        [NodeControl::default(), NodeControl::default()],
    );
    let replacement = Box::new(prepare_processor(&new_execution, &factory));
    let replacement_plan = Arc::clone(replacement.plan());
    exchange
        .submit(replacement)
        .expect("replacement topology should queue");
    assert!(exchange.activate_pending(&mut current));
    exchange.reclaim_retired();

    assert!(Arc::ptr_eq(current.plan(), &replacement_plan));
    assert_eq!(current.plan().revision(), 2);
    assert_eq!(current.compensation_state(), CompensationState::Current);
    assert_eq!(exchange.plan_revisions().active, 2);
    assert_eq!(exchange.plan_revisions().submitted, 2);
    assert!(!exchange.activate_pending(&mut current));

    let output = render_impulse_stream(&mut current, 2, 0, 1.0);
    assert_aligned_impulse(&output, 2, 2, 2.0);
    assert_eq!(factory.counts().prepares.load(Ordering::Relaxed), 2);
}

#[test]
fn destination_off_and_mute_discard_its_queued_compensation_history() {
    for transition in [
        NodeControl {
            enabled: false,
            muted: false,
        },
        NodeControl {
            enabled: true,
            muted: true,
        },
    ] {
        let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
        let execution = diamond_execution(
            1,
            2,
            128,
            false,
            [
                NodeControl {
                    enabled: true,
                    muted: true,
                },
                NodeControl::default(),
            ],
        );
        let mut processor = prepare_processor(&execution, &factory);
        let plan = Arc::clone(processor.plan());
        let exchange = crate::ProcessorExchange::new(1);
        let mut first = [0.0; 2];
        render_without_heap(&mut processor, 1, &[1.0; 2], &mut first);

        publish_control(&exchange, &plan, 2, "mix", transition);
        let mut transition_output = [0.0; 2];
        render_without_heap(&mut processor, 1, &[0.0; 2], &mut transition_output);
        assert_eq!(
            transition_output.map(f32::to_bits),
            [0.0_f32; 2].map(f32::to_bits)
        );

        publish_control(&exchange, &plan, 3, "mix", NodeControl::default());
        let mut resumed = [0.0; 10];
        render_without_heap(&mut processor, 5, &[0.0; 10], &mut resumed);
        assert_eq!(resumed.map(f32::to_bits), [0.0_f32; 10].map(f32::to_bits));
    }
}

#[test]
fn input_timeline_restart_discards_queued_route_history_without_heap_work() {
    for (clock_restart, source_restart) in [(false, false), (true, false), (false, true)] {
        let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
        let execution = diamond_execution(
            1,
            2,
            128,
            false,
            [
                NodeControl {
                    enabled: true,
                    muted: true,
                },
                NodeControl::default(),
            ],
        );
        let mut processor = prepare_processor(&execution, &factory);
        let (_, mut io, mut cycle) = callback_fixture();
        io.value = 1.0;
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));

        io.value = 0.0;
        io.discontinuity = source_restart;
        cycle.first_frame += 4;
        if clock_restart {
            cycle.clock.epoch += 1;
            cycle.discontinuity = true;
        }
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        let result = processor.process_external_cycle(cycle, &mut io);
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        result.unwrap();
        let expected = if clock_restart || source_restart {
            [0.0_f32; 8]
        } else {
            [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]
        };
        assert_eq!(io.output.map(f32::to_bits), expected.map(f32::to_bits));
        assert_eq!(processor.compensation_state(), CompensationState::Current);
    }
}

#[test]
fn unrelated_source_restart_retains_compensated_route_history() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let mixer = node(
        "mix",
        NodeConfig::Mixer(MixerConfig {
            inputs: vec![PortId::from("dry"), PortId::from("wet")],
            channels: 2,
        }),
    );
    let execution = GraphCompiler::new(DelayProviderResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![
                    device_input("source-a"),
                    device_input("source-b"),
                    device_output("sink-a"),
                    device_output("sink-b"),
                    provider_node(
                        2,
                        NodeControl {
                            enabled: true,
                            muted: true,
                        },
                    ),
                    mixer,
                ],
                vec![
                    edge("a", "source-a", AUDIO_PORT_ID, "sink-a", AUDIO_PORT_ID),
                    edge("b-wet", "source-b", AUDIO_PORT_ID, "effect", AUDIO_PORT_ID),
                    edge("wet", "effect", AUDIO_PORT_ID, "mix", "wet"),
                    edge("b-dry", "source-b", AUDIO_PORT_ID, "mix", "dry"),
                    edge("out", "mix", AUDIO_PORT_ID, "sink-b", AUDIO_PORT_ID),
                ],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    let mut processor = prepare_processor(&execution, &factory);
    let (_, mut io, mut cycle) = independent_bus_fixture();
    for _ in 0..3 {
        processor.process_external_cycle(cycle, &mut io).unwrap();
        cycle.first_frame += u64::from(cycle.frames);
    }
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
    io.source_a_discontinuity = true;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
}

#[test]
fn hosted_off_suspends_its_own_delay_history_before_reenable() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = linear_execution(1, 2, 128, NodeControl::default());
    let mut processor = prepare_processor(&execution, &factory);
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);

    let mut first = [0.0; 2];
    render_without_heap(&mut processor, 1, &[1.0; 2], &mut first);
    publish_control(
        &exchange,
        &plan,
        2,
        "effect",
        NodeControl {
            enabled: false,
            muted: false,
        },
    );
    let mut off_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[0.0; 2], &mut off_output);
    publish_control(&exchange, &plan, 3, "effect", NodeControl::default());

    let mut resumed = [0.0; 12];
    render_without_heap(&mut processor, 6, &[0.0; 12], &mut resumed);
    assert_eq!(resumed.map(f32::to_bits), [0.0_f32; 12].map(f32::to_bits));
    assert_eq!(factory.counts().suspensions.load(Ordering::Relaxed), 1);
}

fn measure_delay_case(channels: u16, compensation: bool) -> serde_json::Value {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(1, channels, 32, false, [NodeControl::default(); 2]);
    let mut processor = prepare_processor(&execution, &factory);
    if !compensation {
        processor.delay_bank = None;
    }
    let input = vec![0.01; usize::from(channels) * 32];
    let mut output = vec![0.0; input.len()];
    for _ in 0..500 {
        processor
            .process_block(32, &[&input], &mut [&mut output])
            .unwrap();
    }
    let mut timings = vec![0_u64; 3000];
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    for timing in &mut timings {
        let started = std::time::Instant::now();
        processor
            .process_block(
                32,
                &[std::hint::black_box(&input)],
                &mut [std::hint::black_box(&mut output)],
            )
            .unwrap();
        *timing = u64::try_from(started.elapsed().as_nanos()).unwrap();
        std::hint::black_box(&output);
    }
    let heap_operations = HEAP_OPERATIONS
        .with(|counter| counter.replace(None))
        .unwrap();
    timings.sort_unstable();
    assert_eq!(heap_operations, 0);
    serde_json::json!({
        "channels": channels, "frames": 32, "compensation": compensation,
        "p50_ns": timings[1500], "p95_ns": timings[2850], "p99_ns": timings[2970],
        "max_ns": timings[2999], "heap_operations": heap_operations,
        "over_budget": timings.iter().filter(|&&n| n > 666_666).count(),
    })
}

#[test]
#[ignore = "serial comparative benchmark; execute explicitly in release with no other workloads"]
fn measure_delayed_diamond_render() {
    for channels in [2, 32, 64] {
        for compensation in [false, true] {
            println!("{}", measure_delay_case(channels, compensation));
        }
    }
}

#[test]
fn destination_arrival_includes_compensation_and_rejects_unmeasured_sources() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(1, 1, 32, false, [NodeControl::default(); 2]);
    let processor = prepare_processor(&execution, &factory);
    let plan = processor.plan();
    assert_eq!(
        plan.destination_arrivals(&[Some(0)]).unwrap(),
        vec![Some(TEST_DELAY)]
    );
    assert_eq!(
        plan.destination_arrivals(&[Some(10)]).unwrap(),
        vec![Some(TEST_DELAY + 10)]
    );
    assert_eq!(plan.destination_arrivals(&[None]).unwrap(), vec![None]);
    assert!(plan.destination_arrivals(&[]).is_err());
}

#[test]
fn snapshot_content_arrival_matches_pcm_delay_across_short_callbacks() {
    let execution = compile(
        vec![
            virtual_endpoint("A/native"),
            node("A/snapshot", NodeConfig::DeviceBus { channels: 2 }),
            node(
                "gain",
                NodeConfig::Gain(GainConfig {
                    channels: 2,
                    gain_db: 0.0,
                }),
            ),
            virtual_endpoint("D/native"),
        ],
        vec![
            edge(
                "publish",
                "A/native",
                FROM_OS_PORT_ID,
                "A/snapshot",
                AUDIO_PORT_ID,
            ),
            edge("read", "A/snapshot", AUDIO_PORT_ID, "gain", AUDIO_PORT_ID),
            edge("out", "gain", AUDIO_PORT_ID, "D/native", TO_OS_PORT_ID),
        ],
    );
    let mut processor = prepare_processor(&execution, &DelayFactory::new(0, Some(0)));
    let mut stream = Vec::new();
    for (tick, frames) in [1, 2, 1, 3, 1].into_iter().enumerate() {
        let mut source = [0.0; 8];
        if tick == 0 {
            source[..2].copy_from_slice(&[1.0, 2.0]);
        }
        let mut a = [0.0; 8];
        let mut d = [0.0; 8];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor
            .process_block(frames, &[&source, &[0.0; 8]], &mut [&mut a, &mut d])
            .unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        stream.extend_from_slice(&d[..frames as usize * 2]);
    }
    assert!(processor.acknowledge_content_latency());
    let plan = processor.plan();
    assert_eq!(plan.destination_source_indexes(), vec![vec![], vec![0]]);
    let root = 1_000_000.25;
    let arrival = root + f64::from(FORMAT.block_frames) * 1e9 / 48_000.0;
    assert_eq!(
        plan.measured_destination_arrivals(&[Some(root), None], 1e6)
            .unwrap(),
        vec![None, Some(arrival)]
    );
    assert_eq!(
        plan.measured_destination_arrivals(&[None, None], 1e6)
            .unwrap(),
        vec![None, None]
    );
    assert_aligned_impulse(&stream, 2, FORMAT.block_frames as usize, 1.0);
}

#[test]
fn measured_arrivals_require_acknowledged_hosted_history_and_preserve_fractional_roots() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(1, 1, 32, false, [NodeControl::default(); 2]);
    let mut processor = prepare_processor(&execution, &factory);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![None]
    );
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    assert!(!processor.acknowledge_content_latency());
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    let delay = f64::from(TEST_DELAY) * 1e9 / 48_000.0;
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![Some(delay + 0.25)]
    );
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[None], 1e6)
            .unwrap(),
        vec![None]
    );
    // A read that keeps overlapping the audio thread is disturbed, not unknown.
    processor.plan().compensation.acknowledging(true);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6),
        Err(crate::ArrivalError::Contended)
    );
    processor.plan().compensation.acknowledging(false);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![Some(delay + 0.25)]
    );
    assert!(matches!(
        processor.plan().measured_destination_arrivals(&[], 1e6),
        Err(crate::ArrivalError::Invalid(_))
    ));
    assert_eq!(processor.plan().destination_source_indexes(), vec![vec![0]]);
    render_without_heap(&mut processor, 32, &[0.0; 32], &mut [0.0; 32]);
    assert!(
        !processor.acknowledge_content_latency(),
        "a partial warmup block still taints its window"
    );
    render_without_heap(&mut processor, 32, &[0.0; 32], &mut [0.0; 32]);
    assert!(processor.acknowledge_content_latency());
    factory
        .state
        .reported_frames
        .store(TEST_DELAY + 1, Ordering::Release);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![None]
    );
    processor.delay_bank.as_mut().unwrap().validate();
    assert!(!processor.acknowledge_content_latency());
}

struct PathIo {
    input_bad: bool,
    output_bad: bool,
    discontinuity: bool,
}
impl ExternalIo for PathIo {
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        _: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        TransferOutcome {
            frames: if self.input_bad && binding.node_id().as_str() == "bad" {
                0
            } else {
                cycle.frames
            },
            discontinuity: self.discontinuity && binding.node_id().as_str() == "bad",
            activity: mixrelay_audio_io::process::InputActivity::Active,
            source_sample_faults: 0,
        }
    }
    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        _: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        TransferOutcome {
            frames: if self.output_bad && binding.node_id().as_str() == "bad" {
                0
            } else {
                cycle.frames
            },
            discontinuity: self.discontinuity && binding.node_id().as_str() == "bad",
            activity: mixrelay_audio_io::process::InputActivity::Active,
            source_sample_faults: 0,
        }
    }
}
#[test]
fn binding_latency_faults_and_history_warmup_are_scoped_to_their_path() {
    let execution = compile(
        vec![virtual_endpoint("good"), virtual_endpoint("bad")],
        vec![
            edge("good-path", "good", FROM_OS_PORT_ID, "good", TO_OS_PORT_ID),
            edge("bad-path", "bad", FROM_OS_PORT_ID, "bad", TO_OS_PORT_ID),
        ],
    );
    let mut processor = prepare_processor(&execution, &DelayFactory::new(0, Some(0)));
    let (_, _, cycle) = callback_fixture();
    for (index, (input_bad, output_bad, discontinuity)) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (false, false, false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut io = PathIo {
            input_bad,
            output_bad,
            discontinuity,
        };
        let report = processor
            .process_external_cycle(
                ProcessCycle {
                    first_frame: index as u64 * 4,
                    ..cycle
                },
                &mut io,
            )
            .unwrap()
            .unwrap();
        assert_eq!(report.input_starvations, u64::from(input_bad));
        assert_eq!(report.output_overflows, u64::from(output_bad));
        assert_eq!(report.discontinuities, u64::from(discontinuity) * 2);
        let _ = processor.acknowledge_content_latency();
        assert!(processor.binding_content_latency_ready("good/capture", true));
        assert!(processor.binding_content_latency_ready("good/render", false));
        assert_eq!(
            processor.binding_content_latency_ready("bad", true),
            !input_bad && !discontinuity
        );
        assert_eq!(
            processor.binding_content_latency_ready("bad", false),
            !output_bad && !discontinuity
        );
        assert!(!processor.binding_content_latency_ready("missing", true));
    }
    // Independent route histories can be unprimed without tainting the good path.
    let bad = processor
        .plan
        .compensation
        .binding_routes(&NodeId::from("bad"));
    processor.delay_bank.as_mut().unwrap().reset_routes(&bad);
    assert!(processor.binding_content_latency_ready("good", false));
    assert!(!processor.binding_content_latency_ready("bad", false));
}
