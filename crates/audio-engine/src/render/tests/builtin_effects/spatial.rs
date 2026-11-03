//! Delay, chorus and reverb PCM and history contracts.
use super::*;

#[test]
fn delay_impulse_has_exact_spacing_feedback_and_zero_time_identity() {
    let mut config = configs().remove(8);
    let mut input = vec![0.0; 512];
    input[0] = 1.0;
    let output = signal(&config, &input, None, &[7]);
    assert_eq!(output[96], 1.0);
    assert_eq!(output[192], 0.5);
    assert_eq!(output[288], 0.25);
    assert_eq!(output[..96], [0.0; 96]);
    if let mixrelay_audio_graph::Effect::Delay(p) = &mut config.effect {
        p.time_ms = 0.0;
    }
    assert_eq!(signal(&config, &input, None, &[32]), input);
}

#[test]
fn ping_pong_impulse_alternates_stereo_pairs_without_block_or_channel_order_dependence() {
    let mut config = configs().remove(12);
    config.channels = 4;
    if let mixrelay_audio_graph::Effect::Delay(p) = &mut config.effect {
        p.feedback = 0.5;
    }
    let mut input = vec![0.0; 240 * 4];
    input[..4].copy_from_slice(&[1.0, 1.0, 0.5, 0.5]);
    let output = signal(&config, &input, None, &[7]);
    assert_eq!(output, signal(&config, &input, None, &[1, 31, 128]));
    for (frame, expected) in [
        (48, [1.0, 0.0, 0.5, 0.0]),
        (96, [0.0, 0.5, 0.0, 0.25]),
        (144, [0.25, 0.0, 0.125, 0.0]),
    ] {
        assert_eq!(&output[frame * 4..frame * 4 + 4], &expected);
    }
    assert!(output[..48 * 4].iter().all(|sample| *sample == 0.0));
    config.bypass = true;
    assert_eq!(signal(&config, &input, None, &[128]), input);
    config.bypass = false;
    config.channels = 1;
    let mut mono = vec![0.0; 200];
    mono[0] = 1.0;
    let output = signal(&config, &mono, None, &[31]);
    assert_eq!(output[48], 1.0);
    assert_eq!(output[96], 0.5);
    if let mixrelay_audio_graph::Effect::Delay(p) = &mut config.effect {
        p.time_ms = 0.0;
    }
    assert_eq!(signal(&config, &mono, None, &[31]), mono);
}

#[test]
fn reverb_pre_delay_moves_the_entire_wet_response_by_the_requested_sample_count() {
    for rate in [44_100, 48_000, 96_000] {
        let mut config = configs().remove(10);
        let frames = usize::try_from(rate).unwrap() / 4;
        let mut input = vec![0.0; frames * 2];
        input[..2].fill(1.0);
        let immediate = signal_at_rate(&config, &input, None, &[128], rate);
        if let mixrelay_audio_graph::Effect::Reverb(p) = &mut config.effect {
            p.pre_delay_ms = 20.0;
        }
        let deferred = signal_at_rate(&config, &input, None, &[31, 1, 7], rate);
        let shift = usize::try_from(rate / 50).unwrap() * 2;
        assert!(deferred[..shift].iter().all(|sample| *sample == 0.0));
        assert_eq!(&deferred[shift..], &immediate[..immediate.len() - shift]);
    }
}

#[test]
fn reverb_damping_reduces_high_frequency_tail_energy_and_maximum_feedback_remains_bounded() {
    let mut config = configs().remove(13);
    config.channels = 1;
    if let mixrelay_audio_graph::Effect::Reverb(p) = &mut config.effect {
        p.pre_delay_ms = 0.0;
        p.room_size = 1.0;
        p.damping = 0.0;
    }
    let mut input = vec![0.0; 48_000];
    input[0] = 0.5;
    let bright = signal(&config, &input, None, &[128]);
    if let mixrelay_audio_graph::Effect::Reverb(p) = &mut config.effect {
        p.damping = 0.95;
    }
    let dark = signal(&config, &input, None, &[128]);
    let roughness = |samples: &[f32]| {
        let power = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>();
        samples
            .windows(2)
            .map(|pair| f64::from(pair[1] - pair[0]).powi(2))
            .sum::<f64>()
            / power
    };
    assert!(roughness(&dark[12_000..]) < roughness(&bright[12_000..]) * 0.5);
    if let mixrelay_audio_graph::Effect::Reverb(p) = &mut config.effect {
        p.pre_delay_ms = 200.0;
        p.room_size = 2.0;
        p.decay_seconds = 6.0;
    }
    assert!(
        signal(&config, &input, None, &[31])
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() < 2.0)
    );
    config = configs().remove(12);
    config.channels = 1;
    if let mixrelay_audio_graph::Effect::Delay(p) = &mut config.effect {
        p.feedback = 0.9;
    }
    let echo = signal(&config, &input, None, &[31]);
    assert_eq!(echo[48], 0.5);
    assert!(echo[48 * 100].abs() < 0.0001);
    assert!(
        echo.iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 0.5)
    );
}

#[test]
fn room_and_tunnel_have_different_onsets_decay_tails_and_stereo_diffusion() {
    let mut room = configs().remove(10);
    if let mixrelay_audio_graph::Effect::Reverb(p) = &mut room.effect {
        p.room_size = 0.6;
        p.decay_seconds = 0.4;
        p.pre_delay_ms = 4.0;
        p.damping = 0.6;
        p.algorithm = mixrelay_audio_graph::ReverbAlgorithm::Dense;
    }
    let tunnel = configs().remove(13);
    let mut input = vec![0.0; 96_000 * 2];
    input[..2].fill(1.0);
    let small = signal(&room, &input, None, &[128]);
    let large = signal(&tunnel, &input, None, &[128]);
    let onset = |samples: &[f32]| {
        samples
            .iter()
            .position(|sample| sample.abs() > 1e-6)
            .unwrap()
            / 2
    };
    assert!(onset(&large) > onset(&small) + 3000);
    let energy = |samples: &[f32]| {
        samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
    };
    assert!(energy(&large[48_000 * 2..]) > energy(&small[48_000 * 2..]) * 100.0);
    assert!(
        large
            .chunks_exact(2)
            .any(|pair| (pair[0] - pair[1]).abs() > 0.001)
    );
    assert!(
        large
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() < 2.0)
    );
    let mut left_only = vec![0.0; 12_000 * 2];
    left_only[0] = 1.0;
    let spread = signal(&room, &left_only, None, &[31]);
    assert!(spread.chunks_exact(2).any(|pair| pair[1].abs() > 0.001));
    room.channels = 1;
    let mut mono = vec![0.0; 12_000];
    mono[0] = 1.0;
    let output = signal(&room, &mono, None, &[31]);
    assert!(output.iter().any(|sample| sample.abs() > 0.001));
    assert!(output.iter().all(|sample| sample.is_finite()));
}

#[test]
fn spatial_preset_edits_reuse_history_and_do_not_allocate_or_destroy_in_the_callback() {
    for index in [12, 13] {
        let config = configs().remove(index);
        let mut current = processor(config.clone(), 1, FORMAT);
        let input = [0.25; 8];
        let mut output = [0.0; 8];
        current
            .process_block(4, &[&input], &mut [&mut output])
            .unwrap();
        let mut changed = config;
        match &mut changed.effect {
            mixrelay_audio_graph::Effect::Delay(p) => {
                p.mode = mixrelay_audio_graph::DelayMode::Independent;
                p.time_ms = 1.5;
            }
            mixrelay_audio_graph::Effect::Reverb(p) => {
                p.room_size = 0.5;
                p.pre_delay_ms = 0.0;
                p.decay_seconds = 0.4;
                p.algorithm = mixrelay_audio_graph::ReverbAlgorithm::Classic;
            }
            _ => unreachable!(),
        }
        let replacement = processor(changed, 2, FORMAT);
        let exchange = crate::ProcessorExchange::new(1);
        exchange.submit(replacement).unwrap();
        HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
        assert!(exchange.activate_pending(&mut current));
        let mut retained_tail = false;
        for _ in 0..1000 {
            current
                .process_block(4, &[&[0.0; 8]], &mut [&mut output])
                .unwrap();
            retained_tail |= output.iter().any(|sample| sample.abs() > 1e-8);
        }
        assert!(
            retained_tail,
            "parameter activation must retain spatial history"
        );
        current.effect_states[0].suspend();
        for _ in 0..1000 {
            current
                .process_block(4, &[&[0.0; 8]], &mut [&mut output])
                .unwrap();
            assert_eq!(output, [0.0; 8]);
        }
        assert_eq!(
            HEAP_OPERATIONS.with(|counter| counter.replace(None)),
            Some(0)
        );
        exchange.reclaim_retired();
    }
}
