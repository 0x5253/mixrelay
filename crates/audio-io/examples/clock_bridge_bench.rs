#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a lab tool, not shipped: setup failures abort with their message"
)]
//! Matched release benchmark of the public streaming boundary, without native devices.

use std::{hint::black_box, io::Write, time::Instant};

use mixrelay_audio_io::clock_bridge::{
    AsyncClockBridge, BridgeConfig, ClockFeedback, ClockRelation, FeedbackQueue,
};

const ITERATIONS: u32 = 2_000;
const WARMUP: u32 = 256;
const PASSES: usize = 7;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Bypass,
    Resample,
    Independent,
}

fn bridge(channels: usize, mode: Mode) -> AsyncClockBridge {
    let mut bridge = AsyncClockBridge::new(BridgeConfig {
        channels,
        input_rate: 48_000,
        output_rate: if matches!(mode, Mode::Resample) {
            44_100
        } else {
            48_000
        },
        max_input_frames: 256,
        max_output_frames: 512,
        capacity_frames: 2_048,
        priming_frames: 0,
        clock_relation: if matches!(mode, Mode::Independent) {
            ClockRelation::Independent
        } else {
            ClockRelation::ProvenSameClock
        },
        feedback_queue: FeedbackQueue::Output,
        max_correction_ppm: 1_000.0,
    })
    .expect("valid benchmark geometry");
    if matches!(mode, Mode::Independent) {
        bridge
            .observe_clock(ClockFeedback {
                queue: FeedbackQueue::Output,
                epoch: 0,
                queued_frames: 1_072,
                target_frames: 1_024,
                elapsed_frames: 48_000,
                clock_running: true,
            })
            .expect("valid independent clock observation");
        assert!((bridge.correction_ppm() + 200.0).abs() < f64::EPSILON);
    }
    bridge
}

fn transfer(bridge: &mut AsyncClockBridge, input: &[f32], output: &mut [f32]) -> usize {
    bridge
        .push_interleaved(black_box(input))
        .expect("bounded benchmark input");
    let report = bridge
        .pull_interleaved(black_box(output))
        .expect("bounded benchmark output");
    black_box(output);
    report.transferred_frames
}

fn validate(bridge: &mut AsyncClockBridge, channels: usize, output: &mut [f32]) {
    let input: Vec<f32> = (0..128 * channels)
        .map(|index| f32::from(u16::try_from(index % channels + 1).unwrap()) / 128.0)
        .collect();
    let frames = transfer(bridge, &input, output);
    assert!(frames > 0);
    for frame in output[..frames * channels].chunks_exact(channels) {
        for (channel, sample) in frame.iter().enumerate() {
            let expected = f32::from(u16::try_from(channel + 1).unwrap()) / 128.0;
            assert!((sample - expected).abs() < 0.0001);
        }
    }
}

fn measure(channels: usize, frames: usize, mode: Mode) -> (f64, usize) {
    let mut bridge = bridge(channels, mode);
    let input: Vec<f32> = (0..frames * channels)
        .map(|index| f32::from(u16::try_from(index % channels + 1).unwrap()) / 128.0)
        .collect();
    let mut output = vec![0.0; 512 * channels];
    for _ in 0..WARMUP {
        transfer(&mut bridge, &input, &mut output);
    }
    let mut transferred = 0;
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        transferred += transfer(&mut bridge, &input, &mut output);
    }
    let nanos = start.elapsed().as_secs_f64() * 1e9 / f64::from(ITERATIONS);
    validate(&mut bridge, channels, &mut output);
    assert!(bridge.queued_input_frames() < 128);
    (nanos, transferred)
}

fn main() -> std::io::Result<()> {
    let mut output = std::io::stdout().lock();
    writeln!(
        output,
        "channels,frames,mode,median_ns,min_ns,max_ns,transferred"
    )?;
    for channels in [2, 8, 64] {
        for frames in [32, 256] {
            for mode in [Mode::Bypass, Mode::Resample, Mode::Independent] {
                let results =
                    std::array::from_fn::<_, PASSES, _>(|_| measure(channels, frames, mode));
                assert!(results.iter().all(|result| result.1 == results[0].1));
                let mut nanos = results.map(|result| result.0);
                nanos.sort_by(f64::total_cmp);
                writeln!(
                    output,
                    "{channels},{frames},{mode:?},{:.2},{:.2},{:.2},{}",
                    nanos[PASSES / 2],
                    nanos[0],
                    nanos[PASSES - 1],
                    results[0].1
                )?;
            }
        }
    }
    Ok(())
}
