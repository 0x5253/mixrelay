use super::*;
use std::{hint::black_box, time::Instant};

fn graph(channels: u16, frames: u32, fanout: bool) -> BlockProcessor {
    let mut nodes = vec![
        node(
            "input",
            NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::DefaultInput,
                output_channels: channels,
                gain_db: 0.0,
            }),
        ),
        node(
            "output",
            NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: DeviceSelector::DefaultOutput,
                input_channels: channels,
            }),
        ),
    ];
    let count = if fanout { 8 } else { 16 };
    let mut edges = Vec::new();
    let mut previous = "input".to_owned();
    if fanout {
        nodes.push(node(
            "mix",
            NodeConfig::Mixer(MixerConfig {
                channels,
                inputs: (0..count)
                    .map(|i| PortId::from(format!("bus-{i}").as_str()))
                    .collect(),
            }),
        ));
    }
    for index in 0..count {
        let id = format!("gain-{index}");
        nodes.push(node(
            &id,
            NodeConfig::Gain(GainConfig {
                channels,
                gain_db: 0.0,
            }),
        ));
        edges.push(edge(
            &format!("in-{index}"),
            if fanout { "input" } else { &previous },
            AUDIO_PORT_ID,
            &id,
            AUDIO_PORT_ID,
        ));
        if fanout {
            edges.push(edge(
                &format!("mix-{index}"),
                &id,
                AUDIO_PORT_ID,
                "mix",
                &format!("bus-{index}"),
            ));
        }
        previous = id;
    }
    edges.push(edge(
        "out",
        if fanout { "mix" } else { &previous },
        AUDIO_PORT_ID,
        "output",
        AUDIO_PORT_ID,
    ));
    let execution = GraphCompiler::default()
        .compile(
            &GraphDocument::new(1, nodes, edges),
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: frames,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap()
}

#[test]
fn multichannel_chain_and_fanout_preserve_pcm_without_heap_work() {
    for channels in [2, 8, 16, 32, 64] {
        for fanout in [false, true] {
            let mut processor = graph(channels, 32, fanout);
            let schedule = &processor.plan.schedule;
            let visits: usize = [
                RenderPhase::Sources,
                RenderPhase::Processing,
                RenderPhase::Sinks,
            ]
            .into_iter()
            .map(|phase| schedule.nodes(phase).len())
            .sum();
            assert_eq!(visits, processor.plan.steps.len());
            let input: Vec<_> = (0..usize::from(channels) * 32)
                .map(|i| f32::from(u16::try_from(i % usize::from(channels)).unwrap() + 1) / 1024.0)
                .collect();
            let mut output = vec![0.0; input.len()];
            HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
            for _ in 0..64 {
                processor
                    .process_block(32, &[&input], &mut [&mut output])
                    .unwrap();
            }
            let operations = HEAP_OPERATIONS.with(|counter| counter.replace(None));
            assert_eq!(operations, Some(0));
            for (actual, expected) in output.iter().zip(&input) {
                assert!((actual - expected * if fanout { 8.0 } else { 1.0 }).abs() < 1e-7);
            }
        }
    }
}

#[test]
#[ignore = "serial scaling benchmark; run explicitly with identical profile and no other benchmark"]
fn measure_multichannel_render() {
    let mut results = Vec::new();
    for channels in [2, 8, 16, 32, 64] {
        for frames in [32, 128, 256] {
            for fanout in [false, true] {
                let mut processor = graph(channels, frames, fanout);
                let input = vec![0.01; usize::from(channels) * frames as usize];
                let mut output = vec![0.0; input.len()];
                for _ in 0..500 {
                    processor
                        .process_block(frames, &[&input], &mut [&mut output])
                        .unwrap();
                }
                let mut timings = vec![0_u64; 3000];
                HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
                for timing in &mut timings {
                    let started = Instant::now();
                    processor
                        .process_block(frames, &[black_box(&input)], &mut [black_box(&mut output)])
                        .unwrap();
                    *timing = u64::try_from(started.elapsed().as_nanos()).unwrap();
                    black_box(&output);
                }
                let allocations = HEAP_OPERATIONS
                    .with(|counter| counter.replace(None))
                    .unwrap();
                let deadline = u64::from(frames) * 1_000_000_000 / 48_000;
                let exceeded = timings.iter().filter(|&&value| value > deadline).count();
                timings.sort_unstable();
                let row = serde_json::json!({ "channels": channels, "frames": frames,
                    "topology": if fanout { "fanout8" } else { "chain16" },
                    "p50_ns": timings[1500], "p95_ns": timings[2850], "p99_ns": timings[2970],
                    "max_ns": timings[2999], "deadline_ns": deadline, "over_budget": exceeded,
                    "heap_operations": allocations, "arena_bytes": processor.arena.len() * 4 });
                println!("{row}");
                results.push(row);
            }
        }
    }
    if let Some(path) = std::env::var_os("MIXRELAY_SCALING_OUTPUT") {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        serde_json::to_writer_pretty(file, &results).unwrap();
    }
}
