use super::*;
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, BuiltInNodeResolver, CompileContext,
    CompileLimits, DeviceSelector, MixerConfig, NodeControl, ProviderPorts,
};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const TEST_DELAY: u32 = 5;
const UNKNOWN_LATENCY: u32 = u32::MAX;
const PARTITIONS: [usize; 3] = [1, 32, 128];

mod alignment;
mod arrivals;
mod history;

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
