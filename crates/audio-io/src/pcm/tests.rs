use super::*;
use crate::clock_bridge::{AsyncClockBridge, BridgeConfig, ClockRelation, FeedbackQueue};

#[test]
fn normalization_preserves_all_finite_bits_and_counts_only_replacements_without_allocating() {
    let finite = [
        0.0_f32,
        -0.0,
        f32::from_bits(1),
        -f32::MIN_POSITIVE,
        f32::MAX,
        -0.5,
    ];
    let mut samples = [0.0; 9];
    samples[..6].copy_from_slice(&finite);
    samples[6..].copy_from_slice(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
    let mut faults = 0;
    let measured = allocation_counter::measure(|| {
        faults = sanitize_samples(&mut samples);
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(faults, 3);
    assert_eq!(
        samples[..6]
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        finite.map(f32::to_bits)
    );
    assert_eq!(samples[6..], [0.0; 3]);
    assert_eq!(sanitize_samples(&mut samples), 0);
}

#[test]
fn pending_counter_assigns_each_replacement_once_with_documented_modular_arithmetic() {
    let counter = SampleFaultCounter::default();
    counter.record(3);
    counter.record(7);
    assert_eq!(counter.take(), 10);
    assert_eq!(counter.take(), 0);
    counter.record(u64::MAX - 1);
    counter.record(5);
    assert_eq!(counter.take(), 3);
    counter.record(1);
    assert_eq!(counter.take(), 1);
    assert_eq!(std::mem::size_of::<SampleFaultCounter>(), 8);
}

#[test]
fn repaired_source_continues_through_copy_and_sinc_with_unchanged_history() {
    for relation in [ClockRelation::ProvenSameClock, ClockRelation::Independent] {
        let config = BridgeConfig {
            channels: 2,
            input_rate: 48_000,
            output_rate: 48_000,
            max_input_frames: 128,
            max_output_frames: 128,
            capacity_frames: 1024,
            priming_frames: 32,
            clock_relation: relation,
            feedback_queue: FeedbackQueue::Output,
            max_correction_ppm: 1_000.0,
        };
        let mut bridge = AsyncClockBridge::new(config).unwrap();
        let mut reference = AsyncClockBridge::new(config).unwrap();
        let mut output = [0.0; 256];
        let mut expected = [0.0; 256];
        let mut faults = 0;
        let measured = allocation_counter::measure(|| {
            for block in 0..32 {
                let mut input = [0.25; 256];
                if block == 3 {
                    input[..3].copy_from_slice(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
                }
                faults += sanitize_samples(&mut input);
                reference.push_interleaved(&input).unwrap();
                bridge.push_interleaved(&input).unwrap();
                let report = bridge.pull_interleaved(&mut output).unwrap();
                let reference_report = reference.pull_interleaved(&mut expected).unwrap();
                assert_eq!(
                    report.transferred_frames,
                    reference_report.transferred_frames
                );
                assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
            }
        });
        assert_eq!(measured.count_total, 0);
        assert_eq!(faults, 3);
        assert!(output.iter().any(|sample| *sample > 0.0));
    }
}
