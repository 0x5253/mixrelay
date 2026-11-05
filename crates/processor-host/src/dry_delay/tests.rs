//! Delay alignment, history priming and reset, main-bus routing and allocation-free callbacks.

use super::DryDelay;

use mixrelay_audio_graph::{ProviderBus, ProviderBusKind, ProviderChannelFormat, ProviderPorts};

pub(super) fn bus(id: &str, kind: ProviderBusKind, channels: u16) -> ProviderBus {
    ProviderBus {
        id: id.into(),
        name: id.to_owned(),
        kind,
        channels,
        channel_format: ProviderChannelFormat::Discrete,
    }
}

pub(super) fn ports(inputs: Vec<ProviderBus>, outputs: Vec<ProviderBus>) -> ProviderPorts {
    ProviderPorts {
        layout_id: "fixture-layout".into(),
        inputs,
        outputs,
    }
}

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.to_bits(), expected.to_bits(), "sample {index}");
    }
}

fn assert_silent(samples: &[f32]) {
    for (index, sample) in samples.iter().enumerate() {
        assert_eq!(sample.to_bits(), 0.0_f32.to_bits(), "sample {index}");
    }
}

fn test_frame_value(frames: usize) -> f32 {
    f32::from(u16::try_from(frames).expect("test frame count fits u16"))
}

#[test]
fn starts_silent_and_returns_samples_after_exact_delay() {
    let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
    let mut first = [9.0; 2];
    assert!(delay.advance(&[1.0, 2.0], Some(&mut first), 2));
    assert_silent(&first);

    let mut second = [9.0; 3];
    assert!(delay.advance(&[3.0, 4.0, 5.0], Some(&mut second), 3));
    assert_samples(&second, &[0.0, 1.0, 2.0]);
}

#[test]
fn split_calls_match_full_calls_for_interleaved_samples() {
    let source = [
        0.0, 10.0, 1.0, 11.0, 2.0, 12.0, 3.0, 13.0, 4.0, 14.0, 5.0, 15.0, 6.0, 16.0, 7.0, 17.0,
    ];
    let mut full_delay = DryDelay::new(2, 2, 4).expect("valid delay");
    let mut split_delay = DryDelay::new(2, 2, 4).expect("valid delay");
    let mut full_output = Vec::new();
    for start in [0, 4] {
        let mut block = [0.0; 8];
        assert!(full_delay.advance(&source[start * 2..(start + 4) * 2], Some(&mut block), 4));
        full_output.extend_from_slice(&block);
    }

    let mut split_output = Vec::new();
    let mut start = 0;
    for frames in [1, 3, 2, 2] {
        let mut block = vec![0.0; frames * 2];
        assert!(split_delay.advance(
            &source[start * 2..(start + frames) * 2],
            Some(&mut block),
            frames
        ));
        split_output.extend_from_slice(&block);
        start += frames;
    }
    assert_samples(&split_output, &full_output);
}

#[test]
fn longer_input_and_output_slices_keep_their_unused_suffixes() {
    let mut delay = DryDelay::new(2, 2, 2).expect("valid delay");
    let mut first_output = [7.0; 6];
    assert!(delay.advance(
        &[1.0, 10.0, 2.0, 20.0, 90.0, 91.0],
        Some(&mut first_output),
        2
    ));
    assert_silent(&first_output[..4]);
    assert_samples(&first_output[4..], &[7.0; 2]);

    let mut second_output = [8.0; 6];
    assert!(delay.advance(
        &[3.0, 30.0, 4.0, 40.0, 80.0, 81.0],
        Some(&mut second_output),
        2
    ));
    assert_samples(&second_output[..4], &[1.0, 10.0, 2.0, 20.0]);
    assert_samples(&second_output[4..], &[8.0; 2]);
}

#[test]
fn wrap_preserves_delay_across_many_variable_slices() {
    let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
    let mut frame = 0;
    for frames in [1, 2, 3, 1, 2, 3, 2, 1, 3] {
        let input: Vec<f32> = (0..frames)
            .map(|offset| test_frame_value(frame + offset))
            .collect();
        let mut output = vec![-1.0; frames];
        assert!(delay.advance(&input, Some(&mut output), frames));
        for (offset, actual) in output.iter().enumerate() {
            let current = frame + offset;
            let expected = if current < 3 {
                0.0
            } else {
                test_frame_value(current - 3)
            };
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        frame += frames;
    }
}

#[test]
fn channel_mismatch_keeps_shared_channels_and_silences_extra_outputs() {
    let mut more_outputs = DryDelay::new(2, 3, 2).expect("valid delay");
    let mut ignored = [9.0; 6];
    assert!(more_outputs.advance(&[1.0, 10.0, 2.0, 20.0], Some(&mut ignored), 2));
    assert_silent(&ignored);
    let mut output = [9.0; 6];
    assert!(more_outputs.advance(&[3.0, 30.0, 4.0, 40.0], Some(&mut output), 2));
    assert_samples(&output, &[1.0, 10.0, 0.0, 2.0, 20.0, 0.0]);

    let mut fewer_outputs = DryDelay::new(3, 2, 2).expect("valid delay");
    let mut first = [0.0; 4];
    assert!(fewer_outputs.advance(&[5.0, 50.0, 500.0, 6.0, 60.0, 600.0], Some(&mut first), 2));
    let mut second = [0.0; 4];
    assert!(fewer_outputs.advance(&[7.0, 70.0, 700.0, 8.0, 80.0, 800.0], Some(&mut second), 2));
    assert_samples(&second, &[5.0, 50.0, 6.0, 60.0]);
}

#[test]
fn frame_by_frame_and_block_copy_paths_keep_identical_alignment() {
    // Three inputs to two outputs take the frame-by-frame path; two to two copies blocks.
    let mut mapped = DryDelay::new(3, 2, 5).expect("valid delay");
    let mut direct = DryDelay::new(2, 2, 5).expect("valid delay");
    assert!(!mapped.is_direct_copy());
    assert!(direct.is_direct_copy());
    let mut frame = 0;
    for (block, frames) in [1, 4, 5, 2, 3, 5, 1].into_iter().enumerate() {
        let wide: Vec<f32> = (frame..frame + frames)
            .flat_map(|frame| {
                let value = test_frame_value(frame) + 1.0;
                [value, -value, 1000.0 + value]
            })
            .collect();
        let narrow: Vec<f32> = wide
            .chunks(3)
            .flat_map(|frame| [frame[0], frame[1]])
            .collect();
        if block == 3 {
            mapped.reset();
            direct.reset();
        }
        if block == 5 {
            assert!(mapped.advance(&wide, None, frames));
            assert!(direct.advance(&narrow, None, frames));
        } else {
            let mut mapped_output = vec![7.0; frames * 2];
            let mut direct_output = vec![9.0; frames * 2];
            assert!(mapped.advance(&wide, Some(&mut mapped_output), frames));
            assert!(direct.advance(&narrow, Some(&mut direct_output), frames));
            assert_samples(&mapped_output, &direct_output);
            if block == 2 {
                assert_samples(&direct_output[..2], &[1.0, -1.0]);
            }
        }
        frame += frames;
    }
}

#[test]
fn provider_dry_delay_forwards_only_main_inputs_and_keeps_other_outputs_silent() {
    let ports = ports(
        vec![
            bus("side-in", ProviderBusKind::Sidechain, 1),
            bus("main-in", ProviderBusKind::Main, 2),
            bus("aux-in", ProviderBusKind::Auxiliary, 2),
        ],
        vec![
            bus("aux-out", ProviderBusKind::Auxiliary, 2),
            bus("main-out", ProviderBusKind::Main, 2),
            bus("side-out", ProviderBusKind::Sidechain, 1),
            bus("unmatched-main", ProviderBusKind::Main, 1),
        ],
    );
    let mut delay = DryDelay::for_ports(&ports, 2).expect("valid provider layout");
    assert_eq!(
        delay.mapping,
        Some([None, None, Some(1), Some(2), None, None].to_vec())
    );

    let mut first = [7.0; 6];
    assert!(delay.advance(&[900.0, 10.0, 20.0, 800.0, 801.0], Some(&mut first), 1));
    assert_samples(&first, &[0.0; 6]);
    let mut second = [7.0; 6];
    assert!(delay.advance(&[901.0, 11.0, 21.0, 810.0, 811.0], Some(&mut second), 1));
    assert_samples(&second, &[0.0; 6]);
    let mut third = [7.0; 6];
    assert!(delay.advance(&[902.0, 12.0, 22.0, 820.0, 821.0], Some(&mut third), 1));
    assert_samples(&third, &[0.0, 0.0, 10.0, 20.0, 0.0, 0.0]);
}

#[test]
fn provider_dry_delay_none_reset_and_variable_slices_preserve_only_main_mapping() {
    let ports = ports(
        vec![
            bus("side-in", ProviderBusKind::Sidechain, 1),
            bus("main-in", ProviderBusKind::Main, 2),
        ],
        vec![
            bus("main-out", ProviderBusKind::Main, 2),
            bus("aux-out", ProviderBusKind::Auxiliary, 1),
        ],
    );
    let mut delay = DryDelay::for_ports(&ports, 3).expect("valid provider layout");
    assert!(delay.advance(&[99.0, 1.0, 2.0], None, 1));
    let mut first = [7.0; 6];
    assert!(delay.advance(&[98.0, 3.0, 4.0, 97.0, 5.0, 6.0], Some(&mut first), 2));
    assert_samples(&first, &[0.0; 6]);
    let mut second = [7.0; 3];
    assert!(delay.advance(&[96.0, 7.0, 8.0], Some(&mut second), 1));
    assert_samples(&second, &[1.0, 2.0, 0.0]);

    delay.reset();
    let mut reset = [7.0; 3];
    assert!(delay.advance(&[95.0, 9.0, 10.0], Some(&mut reset), 1));
    assert_samples(&reset, &[0.0; 3]);
    assert!(delay.advance(&[94.0, 11.0, 12.0], None, 1));
    let mut after_reset = [7.0; 3];
    assert!(delay.advance(&[93.0, 13.0, 14.0], Some(&mut after_reset), 1));
    assert_samples(&after_reset, &[0.0; 3]);
    let mut refilled = [7.0; 3];
    assert!(delay.advance(&[92.0, 15.0, 16.0], Some(&mut refilled), 1));
    assert_samples(&refilled, &[9.0, 10.0, 0.0]);
}

#[test]
fn provider_dry_delay_preserves_direct_copy_fast_path_for_single_main_bus() {
    let ports = ProviderPorts::single_bus("stereo", 2, 2);
    let mut delay = DryDelay::for_ports(&ports, 2).expect("valid single-main layout");
    assert!(delay.mapping.is_none());
    let input = [1.0, 10.0, 2.0, 20.0];
    let mut output = [7.0; 4];
    assert!(delay.advance(&input, Some(&mut output), 2));
    assert_samples(&output, &[0.0; 4]);
    assert!(delay.advance(&input, Some(&mut output), 2));
    assert_samples(&output, &input);
}

#[test]
fn mapped_advance_reset_and_reprime_allocate_nothing() {
    let ports = ports(
        vec![
            bus("side-in", ProviderBusKind::Sidechain, 1),
            bus("main-in", ProviderBusKind::Main, 2),
        ],
        vec![
            bus("main-out", ProviderBusKind::Main, 2),
            bus("aux-out", ProviderBusKind::Auxiliary, 1),
        ],
    );
    let mut delay = DryDelay::for_ports(&ports, 4).expect("valid provider layout");
    let input = [
        99.0, 1.0, 2.0, 98.0, 3.0, 4.0, 97.0, 5.0, 6.0, 96.0, 7.0, 8.0,
    ];
    let mut output = [7.0; 12];
    let mut accepted = false;
    let allocations = allocation_counter::measure(|| {
        accepted = delay.advance(&input, Some(&mut output), 4);
        delay.reset();
        accepted &= delay.advance(&input, None, 4);
    });
    assert!(accepted);
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
}

#[test]
fn invalid_provider_topology_is_rejected_before_history_allocation() {
    let invalid = ports(
        vec![
            bus("duplicate", ProviderBusKind::Main, 1),
            bus("duplicate", ProviderBusKind::Sidechain, 1),
        ],
        vec![bus("main-out", ProviderBusKind::Main, 1)],
    );
    assert!(DryDelay::for_ports(&invalid, 2).is_err());
}

#[test]
fn zero_input_generator_fallback_is_silent() {
    let mut delay = DryDelay::new(0, 2, 3).expect("zero-input fallback is valid");
    let mut output = [7.0; 4];
    assert!(delay.advance(&[], Some(&mut output), 2));
    assert_silent(&output);
}

#[test]
fn no_output_call_advances_history() {
    let mut delay = DryDelay::new(1, 1, 2).expect("valid delay");
    assert!(delay.advance(&[5.0, 6.0], None, 2));
    let mut output = [0.0; 2];
    assert!(delay.advance(&[7.0, 8.0], Some(&mut output), 2));
    assert_samples(&output, &[5.0, 6.0]);
}

#[test]
fn reset_after_partial_history_refills_silently_across_variable_slices() {
    let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
    let mut old_output = [9.0; 2];
    assert!(delay.advance(&[90.0, 91.0], Some(&mut old_output), 2));
    assert_silent(&old_output);

    delay.reset();
    let mut first = [9.0; 1];
    assert!(delay.advance(&[10.0], Some(&mut first), 1));
    assert_silent(&first);
    let mut next = [9.0; 2];
    assert!(delay.advance(&[20.0, 30.0], Some(&mut next), 2));
    assert_silent(&next);
    let mut refilled = [9.0; 1];
    assert!(delay.advance(&[40.0], Some(&mut refilled), 1));
    assert_samples(&refilled, &[10.0]);
}

#[test]
fn reset_after_full_history_hides_stale_samples_and_none_primes_ring() {
    let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
    let mut startup = [9.0; 3];
    assert!(delay.advance(&[1.0, 2.0, 3.0], Some(&mut startup), 3));
    assert_silent(&startup);
    let mut steady = [9.0; 1];
    assert!(delay.advance(&[4.0], Some(&mut steady), 1));
    assert_samples(&steady, &[1.0]);

    delay.reset();
    assert!(delay.advance(&[10.0], None, 1));
    let mut second = [9.0; 1];
    assert!(delay.advance(&[20.0], Some(&mut second), 1));
    assert_silent(&second);
    let mut third = [9.0; 1];
    assert!(delay.advance(&[30.0], Some(&mut third), 1));
    assert_silent(&third);
    let mut refilled = [9.0; 1];
    assert!(delay.advance(&[40.0], Some(&mut refilled), 1));
    assert_samples(&refilled, &[10.0]);
}

#[test]
fn zero_input_reset_remains_silent() {
    let mut delay = DryDelay::new(0, 2, 4).expect("zero-input fallback is valid");
    let mut output = [3.0; 4];
    assert!(delay.advance(&[], None, 2));
    delay.reset();
    assert!(delay.advance(&[], Some(&mut output), 2));
    assert_silent(&output);
}

#[test]
fn reset_and_reprime_allocate_nothing_for_supported_channel_counts() {
    for channels in [1, 2, 32, 64] {
        let frames = 8;
        let mut delay = DryDelay::new(channels, channels, frames).expect("valid delay");
        let input = vec![0.5; channels * frames];
        let mut output = vec![7.0; channels * frames];
        assert!(delay.advance(&input, Some(&mut output), frames));
        let mut accepted = false;
        let allocations = allocation_counter::measure(|| {
            delay.reset();
            accepted = delay.advance(&input, Some(&mut output), frames);
        });
        assert!(accepted);
        assert_silent(&output);
        assert_eq!(allocations.count_total, 0);
        assert_eq!(allocations.bytes_total, 0);
    }
}

#[test]
fn invalid_requests_leave_history_and_output_unchanged() {
    let mut delay = DryDelay::new(1, 1, 2).expect("valid delay");
    let mut startup = [0.0; 1];
    assert!(delay.advance(&[1.0], Some(&mut startup), 1));
    let mut output = [77.0; 2];
    assert!(!delay.advance(&[], Some(&mut output), 0));
    assert_samples(&output, &[77.0; 2]);
    assert!(!delay.advance(&[2.0], Some(&mut output), 2));
    assert_samples(&output, &[77.0; 2]);
    assert!(!delay.advance(&[2.0, 3.0, 4.0], Some(&mut output[..1]), 2));
    assert_samples(&output, &[77.0; 2]);
    assert!(!delay.advance(&[2.0, 3.0, 4.0], Some(&mut output), 3));
    assert_samples(&output, &[77.0; 2]);

    let mut valid = [0.0; 1];
    assert!(delay.advance(&[2.0], Some(&mut valid), 1));
    assert_silent(&valid);
    assert!(delay.advance(&[3.0], Some(&mut valid), 1));
    assert_samples(&valid, &[1.0]);
}

#[test]
fn constructor_rejects_zero_outputs_and_zero_frames() {
    assert!(DryDelay::new(1, 0, 1).is_err());
    assert!(DryDelay::new(1, 1, 0).is_err());
}

#[test]
fn callback_allocates_nothing_for_supported_channel_counts() {
    for channels in [1, 2, 32, 64] {
        let frames = 8;
        let mut delay = DryDelay::new(channels, channels, frames).expect("valid delay");
        let input = vec![0.25; channels * frames];
        let mut output = vec![0.0; channels * frames];
        let mut accepted = true;
        let allocations = allocation_counter::measure(|| {
            for _ in 0..32 {
                accepted &= delay.advance(&input, Some(&mut output), frames);
                accepted &= delay.advance(&input, None, frames);
            }
        });
        assert!(accepted);
        assert_eq!(allocations.count_total, 0);
        assert_eq!(allocations.bytes_total, 0);
    }
}
