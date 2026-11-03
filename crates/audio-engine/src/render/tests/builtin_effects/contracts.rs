//! Built-in effect validation, geometry and realtime contracts.
use super::*;

#[test]
fn all_effects_are_partition_invariant_finite_and_compile_roundtrip() {
    for config in configs() {
        let channels = usize::from(config.channels);
        let input: Vec<f32> = (0..48_000 * channels)
            .map(|index| if index < channels { 0.5 } else { 0.0 })
            .collect();
        let full = signal(&config, &input, None, &[1024]);
        assert_eq!(full, signal(&config, &input, None, &[1, 7, 128, 31]));
        assert!(
            full.iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 2.0)
        );
        let mut processor = processor(config.clone(), 1, FORMAT);
        let mut output = [0.0; 8];
        processor
            .process_block(
                4,
                &[&input[..4 * channels]],
                &mut [&mut output[..4 * usize::from(config.output_channels())]],
            )
            .unwrap();
        assert_eq!(
            &output[..4 * usize::from(config.output_channels())],
            &full[..4 * usize::from(config.output_channels())]
        );
        let mut bypass = config;
        bypass.bypass = true;
        let dry = signal(&bypass, &input, None, &[128]);
        for (index, sample) in dry.iter().enumerate() {
            assert_eq!(
                *sample,
                input[index / usize::from(bypass.output_channels()) * channels
                    + index % usize::from(bypass.output_channels()).min(channels)]
            );
        }
    }
}

#[test]
fn channel_pair_and_pan_balance_have_expected_geometry_and_levels() {
    let mut config = configs().remove(5);
    for (mode, expected) in [
        [0.8, -0.2],
        [0.3, 0.3],
        [0.8, 0.8],
        [-0.2, -0.2],
        [-0.2, 0.8],
        [-0.8, 0.2],
    ]
    .into_iter()
    .enumerate()
    {
        if let mixrelay_audio_graph::Effect::ChannelUtility(p) = &mut config.effect {
            p.mode =
                mixrelay_audio_graph::ChannelMode::try_from(u8::try_from(mode).unwrap()).unwrap();
        }
        let output = signal(&config, &[0.8, -0.2], None, &[1]);
        for (sample, expected) in output.iter().zip(expected) {
            assert!((sample - expected).abs() < 1e-6);
        }
    }
    let pan = signal(&configs()[6], &[1.0], None, &[1]);
    assert!((pan[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!((pan[0] * pan[0] + pan[1] * pan[1] - 1.0).abs() < 1e-6);
    assert_eq!(signal(&configs()[7], &[1.0, 1.0], None, &[1]), [0.5, 1.0]);
}

#[test]
fn dense_algorithm_produces_a_different_more_diffuse_tail_than_classic() {
    let mut classic = configs().remove(10);
    classic.channels = 1;
    let mut dense = classic.clone();
    if let mixrelay_audio_graph::Effect::Reverb(p) = &mut dense.effect {
        p.algorithm = mixrelay_audio_graph::ReverbAlgorithm::Dense;
    }
    let mut input = vec![0.0; 24_000];
    input[0] = 1.0;
    let sparse = signal(&classic, &input, None, &[128]);
    let diffuse = signal(&dense, &input, None, &[128]);
    assert_ne!(sparse, diffuse);
    let density = |samples: &[f32]| samples.iter().filter(|sample| sample.abs() > 1e-7).count();
    assert!(density(&diffuse[4_800..14_400]) > density(&sparse[4_800..14_400]));
    assert_eq!(diffuse, signal(&dense, &input, None, &[1, 7, 31]));
}

#[test]
fn invalid_configuration_and_aggregate_capacity_are_rejected() {
    let mut config = configs().remove(8);
    if let mixrelay_audio_graph::Effect::Delay(p) = &mut config.effect {
        p.feedback = f32::NAN;
    }
    assert!(config.validate().is_err());
    config = configs().remove(8);
    config.channels = 64;
    assert!(
        effects::Design::new(
            &config,
            RenderFormat {
                sample_rate: 384_000,
                block_frames: 128
            }
        )
        .is_err()
    );
    let mut pan = configs().remove(6);
    pan.channels = 2;
    assert!(pan.validate().is_err());
    let malformed = r#"{"version":1,"channels":2,"bypass":false,"effect":{"kind":"delay","parameters":{"timeMs":1,"feedback":0,"mix":1,"mode":0,"unexpected":true}}}"#;
    assert!(serde_json::from_str::<EffectConfig>(malformed).is_err());
}
