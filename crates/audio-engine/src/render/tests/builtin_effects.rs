#![allow(clippy::float_cmp, clippy::needless_pass_by_value)] // Exact identity, stereo linkage and timing assertions.
use super::*;
use mixrelay_audio_graph::EffectConfig;

pub(super) fn configs() -> Vec<EffectConfig> {
    [
        (
            "equalizer",
            r#"{"lowDb":6,"midDb":-3,"highDb":3,"lowFrequencyHz":150,"lowQ":0.707,"midFrequencyHz":1000,"midQ":0.707,"highFrequencyHz":6000,"highQ":0.707}"#,
        ),
        ("high_pass", r#"{"frequencyHz":200,"q":0.707}"#),
        ("low_pass", r#"{"frequencyHz":3000,"q":0.707}"#),
        (
            "gate",
            r#"{"thresholdDb":-30,"ratio":4,"attackMs":1,"releaseMs":5,"rangeDb":80}"#,
        ),
        (
            "expander",
            r#"{"thresholdDb":-30,"ratio":2,"attackMs":1,"releaseMs":5,"rangeDb":24}"#,
        ),
        ("channel_utility", r#"{"mode":4}"#),
        ("pan", r#"{"position":0}"#),
        ("balance", r#"{"position":0.5}"#),
        ("delay", r#"{"timeMs":1,"feedback":0.5,"mix":1,"mode":0}"#),
        (
            "ducking",
            r#"{"thresholdDb":-30,"reductionDb":12,"attackMs":0.1,"releaseMs":5}"#,
        ),
        (
            "reverb",
            r#"{"decaySeconds":0.4,"damping":0.5,"mix":1,"roomSize":1,"preDelayMs":0,"algorithm":0}"#,
        ),
        ("chorus", r#"{"rateHz":0.6,"depthMs":3,"mix":1}"#),
        ("delay", r#"{"timeMs":1,"feedback":0.55,"mix":1,"mode":1}"#),
        (
            "reverb",
            r#"{"decaySeconds":4.5,"damping":0.75,"mix":1,"roomSize":2,"preDelayMs":65,"algorithm":1}"#,
        ),
    ]
    .into_iter()
    .map(|(kind, parameters)| {
        serde_json::from_str(&format!(
        r#"{{"version":1,"channels":{},"bypass":false,"effect":{{"kind":"{kind}","parameters":{parameters}}}}}"#,
        if kind == "pan" { 1 } else { 2 }
    )).unwrap()
    })
    .collect()
}

pub(super) fn processor(
    config: EffectConfig,
    revision: u64,
    format: RenderFormat,
) -> Box<BlockProcessor> {
    processor_from_source(config, revision, format, "source")
}

fn processor_from_source(
    config: EffectConfig,
    revision: u64,
    format: RenderFormat,
    source: &str,
) -> Box<BlockProcessor> {
    let graph = GraphDocument::new(
        revision,
        vec![
            node(
                source,
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: config.channels,
                    gain_db: 0.0,
                }),
            ),
            node("fx", NodeConfig::Effect(config.clone())),
            node(
                "sink",
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::DefaultOutput,
                    input_channels: config.output_channels(),
                }),
            ),
        ],
        vec![
            edge("in", source, AUDIO_PORT_ID, "fx", AUDIO_PORT_ID),
            edge("out", "fx", AUDIO_PORT_ID, "sink", AUDIO_PORT_ID),
        ],
    );
    let bytes = serde_json::to_vec(&graph).unwrap();
    let graph = serde_json::from_slice(&bytes).unwrap();
    let execution = GraphCompiler::default()
        .compile(
            &graph,
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

fn signal(
    config: &EffectConfig,
    input: &[f32],
    control: Option<&[f32]>,
    blocks: &[usize],
) -> Vec<f32> {
    signal_at_rate(config, input, control, blocks, 48_000)
}

fn signal_at_rate(
    config: &EffectConfig,
    input: &[f32],
    control: Option<&[f32]>,
    blocks: &[usize],
    rate: u32,
) -> Vec<f32> {
    let design = effects::Design::new(
        config,
        RenderFormat {
            sample_rate: rate,
            block_frames: 1024,
        },
    )
    .unwrap();
    let mut state = effects::State::new(&design).unwrap();
    let channels = usize::from(config.channels);
    let outputs = usize::from(config.output_channels());
    let frames = input.len() / channels;
    let mut arena = vec![0.0; 1024 * (channels + outputs + 1)];
    let input_slot = PortSlot {
        offset: 0,
        channels: config.channels,
    };
    let output_slot = PortSlot {
        offset: 1024 * channels,
        channels: config.output_channels(),
    };
    let control_slot = PortSlot {
        offset: 1024 * (channels + outputs),
        channels: 1,
    };
    let mut result = Vec::new();
    let mut position = 0;
    let mut block = 0;
    while position < frames {
        let count = blocks[block % blocks.len()].min(frames - position);
        arena[..count * channels]
            .copy_from_slice(&input[position * channels..(position + count) * channels]);
        if let Some(control) = control {
            arena[control_slot.offset..control_slot.offset + count]
                .copy_from_slice(&control[position..position + count]);
        }
        state.process(
            &design,
            &mut arena,
            input_slot,
            output_slot,
            control.map(|_| control_slot),
            count,
        );
        result.extend_from_slice(&arena[output_slot.offset..output_slot.offset + count * outputs]);
        position += count;
        block += 1;
    }
    result
}

mod contracts;
mod controls;
mod dynamics;
mod filters;
mod spatial;
