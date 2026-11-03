#![allow(clippy::float_cmp)] // Exact resets and prepared thresholds.
use super::*;

#[test]
fn publication_fences_partial_writes_and_counter_wrap_without_an_initial_false_block() {
    let bank = PeakMeterBank::new(1, 2);
    assert_eq!(bank.publication(), None);
    bank.begin_publication();
    bank.record_interleaved(7, 0, 2, &[0.5, 0.25]);
    assert_eq!(bank.publication(), None);
    assert_eq!(bank.drain_rejection_floor(), 2);
    bank.finish_publication(7);
    assert_eq!(
        bank.publication(),
        Some(MeterPublication {
            sequence: 2,
            revision: 7
        })
    );
    bank.publication_sequence
        .store(u64::MAX - 1, Ordering::Relaxed);
    bank.begin_publication();
    assert_eq!(bank.publication(), None);
    assert_eq!(bank.drain_rejection_floor(), 0);
    bank.finish_publication(8);
    assert_eq!(
        bank.publication(),
        Some(MeterPublication {
            sequence: 0,
            revision: 8
        })
    );
}

#[test]
fn a_new_revision_resets_output_input_and_gain_before_it_is_published() {
    let bank = PeakMeterBank::new(1, 2);
    bank.begin_publication();
    bank.record_interleaved(7, 0, 2, &[0.5, 0.25]);
    bank.record_dynamics(
        7,
        0,
        &[1.0, 0.5],
        DynamicsMeterBlock {
            channels: 2,
            minimum_gain: 0.5,
            threshold_db: -6.0,
        },
    );
    bank.finish_publication(7);
    bank.begin_publication();
    bank.record_interleaved(8, 0, 2, &[0.0, 0.0]);
    assert_eq!(bank.publication(), None);
    bank.record_dynamics(
        8,
        0,
        &[0.0, 0.0],
        DynamicsMeterBlock {
            channels: 2,
            minimum_gain: 1.0,
            threshold_db: -12.0,
        },
    );
    bank.finish_publication(8);
    assert_eq!(bank.take_peaks(8, 0, 2), [0.0, 0.0]);
    let reading = bank.take_dynamics(8, 0, 2).unwrap();
    assert_eq!(reading.input_peaks, [0.0, 0.0]);
    assert_eq!(reading.gain_reduction_db, 0.0);
    assert_eq!(reading.threshold_db, -12.0);
    assert_eq!(bank.take_dynamics(7, 0, 2), None);
}

#[test]
fn interleaved_peaks_ignore_non_finite_samples_and_cover_wide_ports() {
    let bank = PeakMeterBank::new(1, 2);
    bank.record_interleaved(3, 0, 2, &[f32::NAN, -0.5, f32::INFINITY, 0.25, -0.75, -0.0]);
    let peaks = bank.take_peaks(3, 0, 2);
    assert_eq!(peaks, [0.75, 0.5]);
    // Ports wider than one reduction group publish every channel once.
    let channels = 70_u16;
    let frame: Vec<f32> = (0..channels)
        .map(|channel| f32::from(channel) / 100.0)
        .collect();
    let mut published = Vec::new();
    for_each_interleaved_peak(
        &frame.repeat(2),
        channels,
        usize::from(channels),
        |channel, peak| {
            published.push((channel, peak));
        },
    );
    assert_eq!(published.len(), usize::from(channels));
    assert_eq!(published[69], (69, 0.69));
}
