//! Serial before/after qualification of the unchanged single-bus hosted render path.

use std::{hint::black_box, sync::Arc, time::Instant};

use mixrelay_audio_engine::{
    BlockProcessor, HostedAudioProcessor, HostedProcessorFactory, HostedProcessorPrepareRequest,
    PeakMeterBank,
};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint,
    BuiltInNodeResolver, ChannelMapping, CompileContext, CompileLimits, CompiledPort,
    DeviceSelector, EdgeSpec, GraphCompiler, GraphDocument, NodeConfig, NodeResolutionError,
    NodeResolver, NodeSpec, PortDirection, ProviderNodeConfig, RenderFormat, ResolvedNode,
};

struct Resolver(u16);

impl NodeResolver for Resolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        if !matches!(node.config, NodeConfig::Provider(_)) {
            return BuiltInNodeResolver.resolve(node, context);
        }
        Ok(ResolvedNode {
            inputs: vec![CompiledPort::new(
                AUDIO_PORT_ID.into(),
                PortDirection::Input,
                self.0,
            )],
            outputs: vec![CompiledPort::new(
                AUDIO_PORT_ID.into(),
                PortDirection::Output,
                self.0,
            )],
        })
    }
}

struct Attenuator;

impl HostedAudioProcessor for Attenuator {
    fn process(&mut self, input: &[f32], output: &mut [f32], _frames: usize) -> bool {
        for (input, output) in input.iter().zip(output) {
            *output = *input * 0.5;
        }
        true
    }
}

impl HostedProcessorFactory for Attenuator {
    fn prepare(
        &self,
        _: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        Ok(Box::new(Self))
    }
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config,
    }
}

fn edge(source: &str, destination: &str) -> EdgeSpec {
    EdgeSpec {
        id: format!("{source}-{destination}").as_str().into(),
        source: AudioEndpoint {
            node_id: source.into(),
            port_id: AUDIO_PORT_ID.into(),
        },
        destination: AudioEndpoint {
            node_id: destination.into(),
            port_id: AUDIO_PORT_ID.into(),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}

fn prepare(channels: u16, frames: u32) -> Result<BlockProcessor, String> {
    let graph = GraphDocument::new(
        1,
        vec![
            node(
                "in",
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: channels,
                    gain_db: 0.0,
                }),
            ),
            node(
                "effect",
                NodeConfig::Provider(ProviderNodeConfig {
                    type_id: "fixture:attenuator".into(),
                    ports: None,
                    parameters: serde_json::json!({}),
                }),
            ),
            node(
                "out",
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::DefaultOutput,
                    input_channels: channels,
                }),
            ),
        ],
        vec![edge("in", "effect"), edge("effect", "out")],
    );
    let execution = GraphCompiler::new(Resolver(channels))
        .compile(
            &graph,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: frames,
                },
                limits: CompileLimits::default(),
            },
        )
        .map_err(|error| error.to_string())?;
    BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(PeakMeterBank::new(3, usize::from(channels))),
        &Attenuator,
    )
    .map_err(|error| error.to_string())
}

fn measure(channels: u16, frames: u32) -> Result<serde_json::Value, String> {
    let mut processor = prepare(channels, frames)?;
    let samples =
        usize::from(channels) * usize::try_from(frames).map_err(|error| error.to_string())?;
    let input = vec![0.25; samples];
    let mut output = vec![0.0; samples];
    let mut times = Vec::with_capacity(3000);
    for index in 0..3500 {
        let started = Instant::now();
        processor
            .process_block(frames, &[black_box(&input)], &mut [&mut output])
            .map_err(|error| error.to_string())?;
        let elapsed = started.elapsed().as_nanos();
        if output
            .iter()
            .any(|value| value.to_bits() != 0.125_f32.to_bits())
        {
            return Err("hosted PCM mismatch".into());
        }
        if index >= 500 {
            times.push(elapsed);
        }
    }
    times.sort_unstable();
    Ok(
        serde_json::json!({"channels":channels,"frames":frames,"blocks":times.len(),
        "p50Ns":times[1500],"p99Ns":times[2970],"maxNs":times[2999]}),
    )
}

fn main() -> Result<(), String> {
    if std::env::args().nth(1).as_deref() != Some("--run") {
        return Err("run serially with --run in an unmodified release profile".into());
    }
    let mut results = Vec::new();
    for channels in [2, 32, 64] {
        for frames in [32, 128, 256] {
            results.push(measure(channels, frames)?);
        }
    }
    println!(
        "{}",
        serde_json::to_string(&results).map_err(|error| error.to_string())?
    );
    Ok(())
}
