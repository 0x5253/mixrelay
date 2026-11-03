//! Filter frequency response and coefficient-update contracts.
use super::*;

#[test]
fn filters_reject_out_of_band_and_equalizer_has_expected_center_gain() {
    let amplitude = |config: &EffectConfig, hz: f32| {
        #[allow(clippy::cast_precision_loss)]
        let input: Vec<f32> = (0..48_000)
            .flat_map(|frame| {
                let value = (std::f32::consts::TAU * hz * frame as f32 / 48_000.0).sin() * 0.1;
                [value, value]
            })
            .collect();
        let output = signal(config, &input, None, &[128]);
        let power = output[48_000..]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>();
        (power / 48_000.0).sqrt() / (0.1 / 2.0_f64.sqrt())
    };
    let all = configs();
    assert!(amplitude(&all[1], 20.0) < 0.02);
    assert!(amplitude(&all[1], 2000.0) > 0.99);
    assert!(amplitude(&all[2], 15000.0) < 0.04);
    assert!(amplitude(&all[2], 100.0) > 0.99);
    let mut eq = all[0].clone();
    if let mixrelay_audio_graph::Effect::Equalizer(p) = &mut eq.effect {
        p.low_db = 0.0;
        p.high_db = 0.0;
        p.mid_db = 6.0;
    }
    assert!((amplitude(&eq, 1000.0) - 10.0_f64.powf(6.0 / 20.0)).abs() < 0.002);
}

#[test]
fn equalizer_low_and_high_bands_follow_their_frequency_and_q() {
    let amplitude = |config: &EffectConfig, hz: f32| {
        #[allow(clippy::cast_precision_loss)]
        let input: Vec<f32> = (0..48_000)
            .flat_map(|frame| {
                let value = (std::f32::consts::TAU * hz * frame as f32 / 48_000.0).sin() * 0.01;
                [value, value]
            })
            .collect();
        let output = signal(config, &input, None, &[127, 511, 64]);
        (output[48_000..]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            / 48_000.0)
            .sqrt()
            / (0.01 / 2.0_f64.sqrt())
    };
    for low in [true, false] {
        let mut config = configs()[0].clone();
        let center = if low { 400.0 } else { 4_000.0 };
        let mixrelay_audio_graph::Effect::Equalizer(p) = &mut config.effect else {
            unreachable!()
        };
        p.low_db = if low { 12.0 } else { 0.0 };
        p.mid_db = 0.0;
        p.high_db = if low { 0.0 } else { 12.0 };
        p.low_frequency_hz = center;
        p.high_frequency_hz = center;
        p.low_q = 4.0;
        p.high_q = 4.0;
        let expected = 10.0_f64.powf(12.0 / 20.0);
        assert!((amplitude(&config, center) - expected).abs() < 0.002);
        let narrow = amplitude(&config, center * 1.5);
        assert!(narrow < 1.5, "narrow band response: {narrow}");
        let mixrelay_audio_graph::Effect::Equalizer(p) = &mut config.effect else {
            unreachable!()
        };
        if low {
            p.low_q = 0.2;
        } else {
            p.high_q = 0.2;
        }
        let wide = amplitude(&config, center * 1.5);
        assert!(
            wide > narrow * 2.0,
            "Q changes width: narrow {narrow}, wide {wide}"
        );
        let mixrelay_audio_graph::Effect::Equalizer(p) = &mut config.effect else {
            unreachable!()
        };
        if low {
            p.low_frequency_hz = center * 3.0;
            p.low_q = 4.0;
        } else {
            p.high_frequency_hz = center * 3.0;
            p.high_q = 4.0;
        }
        assert!(
            amplitude(&config, center) < 1.15,
            "moving the center moves the actual bell"
        );
        assert!((amplitude(&config, center * 3.0) - expected).abs() < 0.002);
        // Bell bands return to unity at DC; they must not behave as fixed shelves.
        assert!(amplitude(&config, 20.0) < 1.01);
    }
}

#[test]
fn changing_all_equalizer_band_parameters_activates_without_callback_heap_operations() {
    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let config = configs()[0].clone();
    let mut changed = config.clone();
    let mixrelay_audio_graph::Effect::Equalizer(p) = &mut changed.effect else {
        unreachable!()
    };
    p.low_frequency_hz = 330.0;
    p.low_q = 3.0;
    p.low_db = -12.0;
    p.mid_frequency_hz = 2_000.0;
    p.mid_q = 4.0;
    p.mid_db = 6.0;
    p.high_frequency_hz = 8_000.0;
    p.high_q = 0.3;
    p.high_db = 12.0;
    let mut current = processor(config, 1, format);
    let exchange = crate::ProcessorExchange::new(1);
    exchange.submit(processor(changed, 2, format)).unwrap();
    let input = [0.01; 256];
    let mut output = [0.0; 256];
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    assert!(exchange.activate_pending(&mut current));
    for _ in 0..64 {
        current
            .process_block(128, &[&input], &mut [&mut output])
            .unwrap();
    }
    assert_eq!(
        HEAP_OPERATIONS.with(|counter| counter.replace(None)),
        Some(0)
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
}
