#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bounded deterministic DSP fixtures"
)]
#![expect(
    clippy::float_cmp,
    reason = "exact bypass, zeroing and reset equivalence are intentional contracts"
)]

use super::*;
use std::f64::consts::TAU;

mod realtime;
mod timestamps;

#[test]
fn impossible_pcm_capacity_returns_an_error() {
    assert_eq!(
        buffers::zeroed(usize::MAX),
        Err(BridgeError::Allocation {
            samples: usize::MAX
        })
    );
}

#[test]
fn failed_bridge_replacement_reclaims_partial_pcm_and_preserves_active_audio() {
    let mut options = config(2, 48_000, 48_000);
    options.clock_relation = ClockRelation::ProvenSameClock;
    options.priming_frames = 1;
    let mut active = AsyncClockBridge::new(options).unwrap();
    for relation in [ClockRelation::ProvenSameClock, ClockRelation::Independent] {
        for failed in 0..3 {
            let signal: [f32; 256] = std::array::from_fn(|index| index as f32 / 512.0);
            active.push_interleaved(&signal).unwrap();
            let mut attempted = 0;
            let mut replacement = BridgeConfig {
                clock_relation: relation,
                ..options
            };
            replacement.output_rate = if relation == ClockRelation::ProvenSameClock {
                options.input_rate
            } else {
                44_100
            };
            let allocation = allocation_counter::measure(|| {
                let result = AsyncClockBridge::prepare(replacement, |samples| {
                    let reservation = attempted;
                    attempted += 1;
                    if reservation == failed {
                        return Err(BridgeError::Allocation { samples });
                    }
                    buffers::zeroed(samples)
                });
                assert!(matches!(result, Err(BridgeError::Allocation { .. })));
            });
            assert_eq!(attempted, failed + 1);
            assert_eq!((allocation.count_current, allocation.bytes_current), (0, 0));
            let mut output = [0.0; 256];
            assert_eq!(
                active
                    .pull_interleaved(&mut output)
                    .unwrap()
                    .transferred_frames,
                128
            );
            assert_eq!(output, signal);
        }
    }
}

#[test]
fn converter_residence_counts_elapsed_time_only_for_pcm_retained_between_invocations() {
    let mut options = config(1, 48_000, 48_000);
    options.clock_relation = ClockRelation::ProvenSameClock;
    options.priming_frames = 1;
    let mut bridge = AsyncClockBridge::new(options).unwrap();
    let empty = bridge.report(0, 0);
    let allocations = allocation_counter::measure(|| {
        // t=0: a whole chunk enters and leaves in one invocation, with no elapsed time.
        bridge.advance_residence(std::time::Duration::ZERO);
        bridge.push_interleaved(&[0.25; 128]).unwrap();
        let immediate = bridge.pull_interleaved(&mut [0.0; 128]).unwrap();
        assert_eq!(
            immediate
                .input_residence
                .unwrap()
                .mean_since(empty.input_residence.unwrap()),
            Some(0.0)
        );
        assert_eq!(
            immediate
                .output_residence
                .unwrap()
                .mean_since(empty.output_residence.unwrap()),
            Some(0.0)
        );
        // t=128: leave 32 scratch frames and 128 input frames behind.
        bridge.advance_residence(std::time::Duration::from_secs_f64(128.0 / 48_000.0));
        bridge.push_interleaved(&[0.25; 256]).unwrap();
        let partial = bridge.pull_interleaved(&mut [0.0; 96]).unwrap();
        // t=608: those frames have waited 480 input-clock frames = 10 ms.
        bridge.advance_residence(std::time::Duration::from_millis(10));
        let waited = bridge.pull_interleaved(&mut [0.0; 160]).unwrap();
        let input_ns = waited
            .input_residence
            .unwrap()
            .mean_since(partial.input_residence.unwrap())
            .unwrap()
            * 0.5e9
            / 48_000.0;
        let output_ns = waited
            .output_residence
            .unwrap()
            .mean_since(partial.output_residence.unwrap())
            .unwrap()
            * 0.5e9
            / 48_000.0;
        assert_eq!(input_ns, 10_000_000.0);
        // 32 of the 160 output frames waited 10 ms; newly produced frames waited zero.
        assert_eq!(output_ns, 2_000_000.0);
        assert_eq!(waited.algorithmic_delay_frames, 0);
        bridge.reset(2, Discontinuity::StreamRestart);
        let before_reset = waited.output_residence.unwrap();
        assert_eq!(
            bridge.report(0, 0).output_residence,
            Some(ResidenceSnapshot {
                invalid: before_reset.invalid.wrapping_add(1),
                ..before_reset
            })
        );
    });
    assert_eq!(allocations.count_total, 0);
}

#[test]
fn transport_reset_invalidates_spanning_residence_but_recovers_without_clearing_counters() {
    let mut options = config(1, 48_000, 48_000);
    options.clock_relation = ClockRelation::ProvenSameClock;
    let mut bridge = AsyncClockBridge::new(options).unwrap();
    let initial = bridge.report(0, 0);
    assert_eq!(initial.input_residence.unwrap().invalid, 0);
    assert_eq!(initial.output_residence.unwrap().invalid, 0);
    bridge.push_interleaved(&[0.25; 256]).unwrap();
    let before = bridge.pull_interleaved(&mut [0.0; 64]).unwrap();
    assert!(before.queued_input_frames > 0 && before.queued_output_frames > 0);
    let allocations = allocation_counter::measure(|| {
        bridge.reset(1, Discontinuity::TransportLoss);
        bridge.reset(2, Discontinuity::StreamRestart);
        let after = bridge.report(0, 0);
        for (before, after) in [
            (before.input_residence, after.input_residence),
            (before.output_residence, after.output_residence),
        ] {
            let (before, after) = (before.unwrap(), after.unwrap());
            assert_eq!(after.sum, before.sum);
            assert_eq!(after.weight, before.weight);
            assert_eq!(after.invalid, before.invalid + 2);
        }
        bridge.push_interleaved(&[0.5; 128]).unwrap();
        bridge.advance_residence(std::time::Duration::from_millis(5));
        let resumed = bridge.pull_interleaved(&mut [0.0; 128]).unwrap();
        for (old, reset, next, expected) in [
            (
                before.input_residence,
                after.input_residence,
                resumed.input_residence,
                480.0,
            ),
            (
                before.output_residence,
                after.output_residence,
                resumed.output_residence,
                0.0,
            ),
        ] {
            let (old, reset, next) = (old.unwrap(), reset.unwrap(), next.unwrap());
            assert_eq!(next.mean_since(old), None);
            assert_eq!(next.mean_since(reset), Some(expected));
        }
    });
    assert_eq!(allocations.count_total, 0);
}

#[test]
fn authority_feedback_reset_preserves_queued_pcm_and_filter_history_without_allocation() {
    for relation in [ClockRelation::Independent, ClockRelation::ProvenSameClock] {
        let mut options = config(1, 48_000, 48_000);
        options.clock_relation = relation;
        let mut bridge = AsyncClockBridge::new(options).unwrap();
        let mut reference = AsyncClockBridge::new(options).unwrap();
        let input: Vec<f32> = (0..1_024)
            .map(|frame| (TAU * 997.0 * f64::from(frame) / 48_000.0).sin() as f32 * 0.25)
            .collect();
        for converter in [&mut bridge, &mut reference] {
            converter.push_interleaved(&input).unwrap();
            converter.pull_interleaved(&mut [0.0; 321]).unwrap();
        }
        let queued = (bridge.queued_input_frames(), bridge.queued_output_frames());
        let allocated = allocation_counter::measure(|| bridge.reset_clock_feedback().unwrap());
        assert_eq!(allocated.count_total, 0);
        assert_eq!(
            queued,
            (bridge.queued_input_frames(), bridge.queued_output_frames())
        );
        assert!(bridge.primed && bridge.started);
        assert_eq!(bridge.epoch, 0);
        let (mut actual, mut expected) = ([0.0; 512], [0.0; 512]);
        let actual_frames = bridge
            .pull_interleaved(&mut actual)
            .unwrap()
            .transferred_frames;
        let expected_frames = reference
            .pull_interleaved(&mut expected)
            .unwrap()
            .transferred_frames;
        assert_eq!(actual_frames, expected_frames);
        assert_eq!(
            actual, expected,
            "nominal authority changes preserve the exact tail"
        );
    }
}

#[test]
fn authority_feedback_reset_clears_drift_but_retains_the_transport_epoch() {
    let mut bridge = AsyncClockBridge::new(config(1, 48_000, 48_000)).unwrap();
    bridge.reset(7, Discontinuity::StreamRestart);
    bridge
        .observe_clock(ClockFeedback {
            queue: FeedbackQueue::Output,
            epoch: 7,
            queued_frames: 0,
            target_frames: 1_024,
            elapsed_frames: 48_000,
            clock_running: true,
        })
        .unwrap();
    assert!(bridge.correction_ppm() > 0.0);
    bridge.push_interleaved(&[0.25; 512]).unwrap();
    bridge.reset_clock_feedback().unwrap();
    assert_eq!(bridge.correction_ppm(), 0.0);
    assert_eq!(bridge.epoch, 7);
    assert_eq!(bridge.queued_input_frames(), 512);
}

#[test]
fn processing_quantum_covers_actual_converter_batches_but_not_same_clock_copies() {
    for (from, to) in [(96_000, 48_000), (48_000, 96_000), (48_000, 48_000)] {
        let bridge = AsyncClockBridge::new(config(1, from, to)).unwrap();
        let (input, output) = bridge.processing_quantum();
        assert_eq!(input, CHUNK);
        assert!(output >= (input * to as usize).div_ceil(from as usize));
        assert_eq!(output, bridge.output_capacity_frames);
    }
    let mut options = config(1, 48_000, 48_000);
    options.clock_relation = ClockRelation::ProvenSameClock;
    assert_eq!(
        AsyncClockBridge::new(options).unwrap().processing_quantum(),
        (1, 1)
    );
}

fn config(channels: usize, input_rate: u32, output_rate: u32) -> BridgeConfig {
    BridgeConfig {
        channels,
        input_rate,
        output_rate,
        max_input_frames: 4096,
        max_output_frames: 4096,
        capacity_frames: 8192,
        priming_frames: 0,
        clock_relation: ClockRelation::Independent,
        feedback_queue: FeedbackQueue::Output,
        max_correction_ppm: 1000.0,
    }
}

#[test]
fn bypass_requires_clock_proof_not_equal_nominal_rates() {
    let mut independent = AsyncClockBridge::new(config(1, 48_000, 48_000)).unwrap();
    let mut c = config(1, 48_000, 48_000);
    c.clock_relation = ClockRelation::ProvenSameClock;
    let mut locked = AsyncClockBridge::new(c).unwrap();
    let input = [0.5; 23];
    let mut output = [9.0; 23];
    locked.push_interleaved(&input).unwrap();
    let report = locked.pull_interleaved(&mut output).unwrap();
    assert_eq!(report.transferred_frames, 23);
    assert_eq!(report.algorithmic_delay_frames, 0);
    assert_eq!(output, input);
    independent.push_interleaved(&input).unwrap();
    let report = independent.pull_interleaved(&mut output).unwrap();
    assert_eq!(report.transferred_frames, 0);
    assert!(report.algorithmic_delay_frames > 0);
    assert_eq!(output, [0.0; 23]);
    c.output_rate = 44_100;
    assert!(AsyncClockBridge::new(c).unwrap().resampler.is_some());
}

#[test]
fn bypass_chunk_uses_only_available_frames_up_to_the_chunk_boundary() {
    for channels in [2, 8, 64] {
        for frames in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, CHUNK * 2] {
            let mut config = config(channels, 48_000, 48_000);
            config.clock_relation = ClockRelation::ProvenSameClock;
            let mut bridge = AsyncClockBridge::new(config).unwrap();
            let samples: Vec<f32> = (0..frames * channels)
                .map(|sample| f32::from(u16::try_from(sample).unwrap()) / 32_768.0)
                .collect();
            bridge.push_interleaved(&samples).unwrap();

            assert_eq!(bridge.convert_chunk().unwrap(), frames > 0);

            let converted = frames.min(CHUNK);
            assert_eq!(bridge.output_frames, converted);
            assert_eq!(bridge.output_read, 0);
            assert_eq!(bridge.started, frames > 0);
            assert_eq!(bridge.input.frames(), frames - converted);
            assert_eq!(
                bridge.output_scratch[..converted * channels],
                samples[..converted * channels]
            );
        }
    }
}

#[test]
fn incomplete_sinc_chunk_keeps_pcm_and_output_cursor_unchanged() {
    let mut bridge = AsyncClockBridge::new(config(2, 48_000, 44_100)).unwrap();
    let needed = bridge.resampler.as_ref().unwrap().input_frames_next();
    bridge
        .push_interleaved(&vec![0.75; (needed - 1) * 2])
        .unwrap();
    bridge.output_scratch.fill(0.25);
    bridge.output_frames = 7;
    bridge.output_read = 7;
    bridge.started = true;

    assert!(!bridge.convert_chunk().unwrap());

    assert_eq!(bridge.input.frames(), needed - 1);
    assert_eq!(bridge.output_frames, 7);
    assert_eq!(bridge.output_read, 7);
    assert!(bridge.started);
    assert!(bridge.output_scratch.iter().all(|sample| *sample == 0.25));
}

fn convert_tone(channels: usize, from: u32, to: u32, frequency: f64) -> Vec<f32> {
    let mut bridge = AsyncClockBridge::new(config(channels, from, to)).unwrap();
    let quantums = [1, 17, 128, 257, 63, 511, 79];
    let mut input = vec![0.0; 511 * channels];
    let mut output = vec![0.0; 4096 * channels];
    let mut result = Vec::new();
    let mut position = 0;
    for iteration in 0..100 {
        let frames = quantums[iteration % quantums.len()];
        for frame in 0..frames {
            input[frame * channels] = (0.5
                * (TAU * frequency * (position + frame) as f64 / f64::from(from)).sin())
                as f32;
        }
        position += frames;
        assert_eq!(
            bridge
                .push_interleaved(&input[..frames * channels])
                .unwrap(),
            frames
        );
        let report = bridge.pull_interleaved(&mut output).unwrap();
        result.extend_from_slice(&output[..report.transferred_frames * channels]);
    }
    result
}

#[test]
fn sinc_preserves_44100_48000_tones_and_channel_isolation() {
    for (from, to) in [(44_100, 48_000), (48_000, 44_100)] {
        for channels in [1, 2, 8, 16, 32, 64] {
            let output = convert_tone(channels, from, to, 997.0);
            let frames = output.len() / channels;
            let (mut cosine, mut sine, mut power) = (0.0, 0.0, 0.0);
            for frame in 1024..frames {
                let sample = f64::from(output[frame * channels]);
                let phase = TAU * 997.0 * frame as f64 / f64::from(to);
                cosine += sample * phase.cos();
                sine += sample * phase.sin();
                power += sample * sample;
                assert!(
                    output[frame * channels + 1..(frame + 1) * channels]
                        .iter()
                        .all(|sample| *sample == 0.0)
                );
            }
            let count = (frames - 1024) as f64;
            let amplitude = 2.0 * cosine.hypot(sine) / count;
            let rms = (power / count).sqrt();
            assert!(
                (amplitude - 0.5).abs() < 0.001,
                "{from}->{to} {channels}ch amplitude={amplitude}"
            );
            assert!((rms - 0.5 / 2_f64.sqrt()).abs() < 0.001);
            eprintln!(
                "tone {from}->{to} {channels}ch amplitude={amplitude:.6} rms={rms:.6} frames={frames}"
            );
        }
    }
}

#[test]
fn downsampling_rejects_out_of_band_aliases() {
    let output = convert_tone(1, 96_000, 44_100, 30_000.0);
    let tail = &output[1024..];
    let rms = (tail
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / tail.len() as f64)
        .sqrt();
    assert!(rms < 0.000_01, "alias rms={rms}");
    eprintln!(
        "96k->44.1k 30kHz rejection: rms={rms:.9}; unfiltered 0.5-amplitude tone rms={:.9}",
        0.5 / 2_f64.sqrt()
    );
}

#[test]
fn reset_clears_ratio_pcm_tail_controller_and_rejects_old_epoch() {
    let c = config(2, 44_100, 48_000);
    let mut used = AsyncClockBridge::new(c).unwrap();
    let feedback = ClockFeedback {
        queue: FeedbackQueue::Output,
        epoch: 0,
        queued_frames: 4000,
        target_frames: 1000,
        elapsed_frames: 480,
        clock_running: true,
    };
    for _ in 0..500 {
        used.observe_clock(feedback).unwrap();
    }
    used.push_interleaved(&[1.0; 1024]).unwrap();
    used.pull_interleaved(&mut [0.0; 300]).unwrap();
    assert!(used.correction_ppm() < -100.0);
    used.reset(3, Discontinuity::StreamRestart);
    assert_eq!(used.correction_ppm(), 0.0);
    assert_eq!(used.queued_input_frames(), 0);
    assert_eq!(
        used.observe_clock(feedback),
        Err(BridgeError::InvalidFeedback)
    );
    let mut fresh = AsyncClockBridge::new(c).unwrap();
    let input = [0.25; 1024];
    used.push_interleaved(&input).unwrap();
    fresh.push_interleaved(&input).unwrap();
    let (mut left, mut right) = ([0.0; 1000], [0.0; 1000]);
    let a = used.pull_interleaved(&mut left).unwrap();
    let b = fresh.pull_interleaved(&mut right).unwrap();
    assert_eq!(a.transferred_frames, b.transferred_frames);
    assert_eq!(left, right);
    assert_eq!(a.epoch, 3);
}

#[test]
fn bounded_buffers_priming_stall_and_feedback_validation() {
    let mut c = config(2, 48_000, 48_000);
    c.priming_frames = 256;
    let mut bridge = AsyncClockBridge::new(c).unwrap();
    assert_eq!(
        bridge.push_interleaved(&[0.0]),
        Err(BridgeError::InvalidBuffer)
    );
    assert_eq!(
        bridge.push_interleaved(&[f32::NAN; 2]),
        Err(BridgeError::InvalidBuffer)
    );
    bridge.push_interleaved(&[0.5; 128]).unwrap();
    let mut output = [99.0; 128];
    let report = bridge.pull_interleaved(&mut output).unwrap();
    assert_eq!(report.state, TransferState::Priming);
    assert_eq!(report.transferred_frames, 0);
    assert_eq!(output, [0.0; 128]);
    let large = vec![0.0; 8192];
    bridge.push_interleaved(&large).unwrap();
    assert_eq!(bridge.push_interleaved(&large), Err(BridgeError::Overflow));
    assert_eq!(bridge.queued_input_frames(), 4160);
    let feedback = ClockFeedback {
        queue: FeedbackQueue::Output,
        epoch: 0,
        queued_frames: 5000,
        target_frames: 1000,
        elapsed_frames: 0,
        clock_running: false,
    };
    for _ in 0..1000 {
        assert_eq!(bridge.observe_clock(feedback).unwrap(), 0.0);
    }
    assert_eq!(
        bridge.observe_clock(ClockFeedback {
            clock_running: true,
            ..feedback
        }),
        Err(BridgeError::InvalidFeedback)
    );
    assert_eq!(
        bridge.observe_clock(ClockFeedback {
            queue: FeedbackQueue::Input,
            ..feedback
        }),
        Err(BridgeError::InvalidFeedback)
    );
}

#[test]
fn external_fifo_feedback_tracks_positive_and_negative_clock_drift() {
    // Actual SRC processes silent PCM, proving that clock feedback does not
    // depend on audio amplitude. Only the simulated FIFO stores frame counts.
    for ppm in [-500.0, -100.0, 100.0, 500.0] {
        let mut bridge = AsyncClockBridge::new(config(1, 48_000, 48_000)).unwrap();
        let mut nominal = AsyncClockBridge::new(config(1, 48_000, 48_000)).unwrap();
        let (mut queue, target) = (1024.0_f64, 1024_usize);
        let mut nominal_balance = queue;
        let (mut minimum, mut maximum) = (queue, queue);
        let mut fractional = 0.0_f64;
        let mut output = [0.0; 256];
        // Startup filter latency is established before beginning clock tracking.
        for _ in 0..4 {
            bridge.push_interleaved(&[0.0; 128]).unwrap();
            bridge.pull_interleaved(&mut output).unwrap();
            nominal.push_interleaved(&[0.0; 128]).unwrap();
            nominal.pull_interleaved(&mut output).unwrap();
        }
        for iteration in 0..67_500 {
            bridge.push_interleaved(&[0.0; 128]).unwrap();
            queue += bridge
                .pull_interleaved(&mut output)
                .unwrap()
                .transferred_frames as f64;
            nominal.push_interleaved(&[0.0; 128]).unwrap();
            nominal_balance += nominal
                .pull_interleaved(&mut output)
                .unwrap()
                .transferred_frames as f64;
            fractional += 128.0 / (1.0 + ppm * 1e-6);
            let consumed = fractional.floor() as usize;
            fractional -= consumed as f64;
            queue -= consumed as f64;
            nominal_balance -= consumed as f64;
            minimum = minimum.min(queue);
            maximum = maximum.max(queue);
            assert!(
                (0.0..4096.0).contains(&queue),
                "ppm={ppm} iteration={iteration} queue={queue}"
            );
            bridge
                .observe_clock(ClockFeedback {
                    queue: FeedbackQueue::Output,
                    epoch: 0,
                    queued_frames: queue as usize,
                    target_frames: target,
                    elapsed_frames: consumed,
                    clock_running: true,
                })
                .unwrap();
        }
        let expected = -ppm / (1.0 + ppm * 1e-6);
        assert!(
            (bridge.correction_ppm() - expected).abs() < 15.0,
            "{ppm}: {}",
            bridge.correction_ppm()
        );
        assert!((queue - target as f64).abs() < 5.0);
        let uncorrected = nominal_balance - target as f64;
        assert!(uncorrected.abs() >= 860.0);
        eprintln!(
            "180s {ppm:+}ppm: FIFO {minimum}..{maximum}, final={queue}, correction={:.3}ppm; same SRC with correction disabled measured frame balance drift={uncorrected:+}",
            bridge.correction_ppm()
        );
    }
}

#[test]
fn controller_saturation_is_bounded_and_recovers_without_integral_windup() {
    let mut bridge = AsyncClockBridge::new(config(1, 48_000, 48_000)).unwrap();
    let mut feedback = ClockFeedback {
        queue: FeedbackQueue::Output,
        epoch: 0,
        queued_frames: 20_000,
        target_frames: 1024,
        elapsed_frames: 480,
        clock_running: true,
    };
    for _ in 0..10_000 {
        let ppm = bridge.observe_clock(feedback).unwrap();
        assert!((-1000.0..=1000.0).contains(&ppm));
    }
    assert_eq!(bridge.correction_ppm(), -1000.0);
    feedback.queued_frames = feedback.target_frames;
    for _ in 0..500 {
        bridge.observe_clock(feedback).unwrap();
    }
    assert!(
        bridge.correction_ppm().abs() < 0.001,
        "saturated integral persisted"
    );
}

#[test]
fn input_feedback_authority_obeys_source_frame_units_and_sign() {
    let mut c = config(2, 44_100, 48_000);
    c.feedback_queue = FeedbackQueue::Input;
    let mut bridge = AsyncClockBridge::new(c).unwrap();
    let feedback = ClockFeedback {
        queue: FeedbackQueue::Input,
        epoch: 0,
        queued_frames: 1124,
        target_frames: 1024,
        elapsed_frames: 441,
        clock_running: true,
    };
    for _ in 0..100 {
        bridge.observe_clock(feedback).unwrap();
    }
    assert!(
        bridge.correction_ppm() < 0.0,
        "full input FIFO must consume more input per output"
    );
    assert_eq!(
        bridge.observe_clock(ClockFeedback {
            elapsed_frames: 44_101,
            ..feedback
        }),
        Err(BridgeError::InvalidFeedback)
    );
}

#[test]
fn heterogeneous_quantums_and_real_rate_conversion_remain_bounded_with_drift() {
    for (from, to, ppm) in [(44_100, 48_000, 500.0), (48_000, 44_100, -500.0)] {
        let mut bridge = AsyncClockBridge::new(config(1, from, to)).unwrap();
        let mut input = [0.0; 511];
        let mut output = [0.0; 1024];
        // Prefill belongs to the external transport, represented here by its
        // exact frame balance; no samples are invented by the converter.
        let mut queue = 2048.0_f64;
        let mut fraction = 0.0_f64;
        let mut frames_elapsed = 0_usize;
        let mut index = 0_usize;
        let (mut low, mut high) = (queue, queue);
        while frames_elapsed < 180 * from as usize {
            let frames = [37, 256, 73, 511, 128, 19][index % 6];
            for sample in &mut input[..frames] {
                *sample = 0.0;
            }
            bridge.push_interleaved(&input[..frames]).unwrap();
            loop {
                let report = bridge.pull_interleaved(&mut output).unwrap();
                queue += report.transferred_frames as f64;
                if report.transferred_frames < output.len() {
                    break;
                }
            }
            fraction += frames as f64 * f64::from(to) / f64::from(from) / (1.0 + ppm * 1e-6);
            let consumed = fraction.floor() as usize;
            fraction -= consumed as f64;
            queue -= consumed as f64;
            assert!(
                (0.0..4096.0).contains(&queue),
                "{from}->{to} {ppm}ppm queue={queue}"
            );
            low = low.min(queue);
            high = high.max(queue);
            bridge
                .observe_clock(ClockFeedback {
                    queue: FeedbackQueue::Output,
                    epoch: 0,
                    queued_frames: queue as usize,
                    target_frames: 2048,
                    elapsed_frames: consumed,
                    clock_running: true,
                })
                .unwrap();
            frames_elapsed += frames;
            index += 1;
        }
        assert!(low > 1600.0 && high < 2500.0);
        assert!((queue - 2048.0).abs() < 150.0);
        eprintln!(
            "variablequantum 180s {from}->{to} {ppm:+}ppm: FIFO {low}..{high}, final={queue}, correction={:.3}ppm",
            bridge.correction_ppm()
        );
    }
}
