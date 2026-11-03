//! Serial, same-build control-state comparison. Run with `cargo run --release
//! -p mixrelay-audio-engine --example node_control_cost`, without custom RUSTFLAGS.
//! Measures block-processing elapsed time, not device latency. Preparation is outside timing.

use std::{error::Error, hint::black_box, io::Write, sync::Arc, time::Instant};

use mixrelay_audio_engine::{BlockProcessor, RenderPlan};
use mixrelay_audio_graph::{
    BuiltInNodeResolver, CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeControl,
    RenderFormat,
};
use serde_json::json;

const FRAMES: u32 = 256;
const BLOCKS: u32 = 10_000;
const ROUNDS: usize = 9;
const MODES: [(&str, NodeControl); 3] = [
    (
        "on",
        NodeControl {
            enabled: true,
            muted: false,
        },
    ),
    (
        "off",
        NodeControl {
            enabled: false,
            muted: false,
        },
    ),
    (
        "mute",
        NodeControl {
            enabled: true,
            muted: true,
        },
    ),
];

fn graph(control: NodeControl) -> Result<GraphDocument, serde_json::Error> {
    let mut nodes = vec![json!({
        "id":"input", "type":"audio_device_input",
        "config":{"selector":{"match":"default_input"},"outputChannels":2}
    })];
    for index in 0..4 {
        nodes.push(json!({
            "id":format!("compressor-{index}"), "type":"compressor", "control":control,
            "config":{"version":1,"bypass":false,"channels":2,"thresholdDb":-18.0,"ratio":4.0,
                "attackMs":5.0,"releaseMs":100.0,"makeupGainDb":3.0}
        }));
        nodes.push(json!({
            "id":format!("limiter-{index}"), "type":"limiter", "control":control,
            "config":{"version":1,"bypass":false,"channels":2,"ceilingDb":-3.0,"releaseMs":100.0}
        }));
    }
    nodes.push(json!({
        "id":"output", "type":"audio_device_output",
        "config":{"selector":{"match":"default_output"},"inputChannels":2}
    }));
    let edges: Vec<_> = nodes
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            json!({
                "id":format!("route-{index}"),
                "source":{"nodeId":pair[0]["id"],"portId":"audio"},
                "destination":{"nodeId":pair[1]["id"],"portId":"audio"}
            })
        })
        .collect();
    serde_json::from_value(json!({"schemaVersion":2,"revision":1,"nodes":nodes,"edges":edges}))
}

fn processor(control: NodeControl) -> Result<BlockProcessor, Box<dyn Error>> {
    let execution = GraphCompiler::new(BuiltInNodeResolver).compile(
        &graph(control)?,
        CompileContext {
            format: RenderFormat {
                sample_rate: 48_000,
                block_frames: FRAMES,
            },
            limits: CompileLimits::default(),
        },
    )?;
    Ok(BlockProcessor::new(Arc::new(RenderPlan::prepare(
        &execution,
    )?))?)
}

fn sample(control: NodeControl) -> Result<u128, Box<dyn Error>> {
    let mut processor = processor(control)?;
    let input: [f32; 512] = std::array::from_fn(|index| if index % 32 < 16 { 0.8 } else { -0.2 });
    let mut output = [0.0; 512];
    for _ in 0..100 {
        processor.process_block(FRAMES, &[&input], &mut [&mut output])?;
    }
    let started = Instant::now();
    for _ in 0..BLOCKS {
        processor.process_block(FRAMES, &[black_box(&input)], &mut [&mut output])?;
        black_box(&output);
    }
    Ok(started.elapsed().as_nanos() / u128::from(BLOCKS))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut timings = [[0_u128; MODES.len()]; ROUNDS];
    // Rotate modes between rounds so one state does not always get the cold machine.
    for (round, samples) in timings.iter_mut().enumerate() {
        for (mode, (_, control)) in MODES
            .iter()
            .enumerate()
            .cycle()
            .skip(round % MODES.len())
            .take(MODES.len())
        {
            samples[mode] = sample(*control)?;
        }
    }
    let mut output = std::io::stdout().lock();
    writeln!(
        output,
        "48000 Hz, 256 frames, stereo, 4 compressor + 4 limiter, {BLOCKS} blocks/round, {ROUNDS} rounds"
    )?;
    for (mode, (name, _)) in MODES.into_iter().enumerate() {
        let mut samples: [_; ROUNDS] = std::array::from_fn(|round| timings[round][mode]);
        samples.sort_unstable();
        writeln!(
            output,
            "{name}: median {} ns/block; sorted samples {samples:?}",
            samples[ROUNDS / 2]
        )?;
    }
    Ok(())
}
