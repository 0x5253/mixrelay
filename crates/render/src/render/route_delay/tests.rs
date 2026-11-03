use super::{MAX_HISTORY_BYTES, RouteDelayError, RouteDelayLine};

fn assert_same_bits(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

fn expected_from_start(input: &[f32], channels: usize, delay_frames: usize) -> Vec<f32> {
    let frames = input.len() / channels;
    let mut expected = vec![0.0; input.len()];
    for frame in delay_frames..frames {
        let source_start = (frame - delay_frames) * channels;
        let destination_start = frame * channels;
        expected[destination_start..destination_start + channels]
            .copy_from_slice(&input[source_start..source_start + channels]);
    }
    expected
}

#[test]
fn callback_partitions_and_ring_wraps_preserve_interleaved_pcm() {
    let delay_frames = 5;
    let partitions = [1, 32, 128];
    let total_frames: usize = partitions.iter().sum();

    for channels in [2, 32, 64] {
        let input: Vec<f32> = (0..total_frames * channels)
            .map(|sample| f32::from(u16::try_from(sample + 1).expect("test sample fits in u16")))
            .collect();
        let expected = expected_from_start(&input, channels, delay_frames);
        let mut output = vec![-1.0; input.len()];
        let mut line = RouteDelayLine::new(channels, delay_frames).expect("valid delay line");

        let mut frame_offset = 0;
        for callback_frames in partitions {
            let sample_offset = frame_offset * channels;
            let sample_count = callback_frames * channels;
            assert!(line.advance(
                &input[sample_offset..sample_offset + sample_count],
                &mut output[sample_offset..sample_offset + sample_count],
                callback_frames,
            ));
            frame_offset += callback_frames;
        }

        assert_same_bits(&output, &expected);
    }
}

#[test]
fn delayed_pcm_preserves_signed_zero_subnormal_and_nan_payload_bits() {
    let input = [
        f32::from_bits(0x8000_0000),
        f32::from_bits(0x7fc1_2345),
        f32::from_bits(0x0000_0001),
        f32::from_bits(0xffc5_4321),
        3.25,
        -7.5,
        9.0,
        11.0,
    ];
    let mut output = [1.0; 8];
    let mut line = RouteDelayLine::new(2, 2).expect("valid delay line");

    assert!(line.advance(&input[..4], &mut output[..4], 2));
    assert_same_bits(&output[..4], &[0.0; 4]);
    assert!(line.advance(&input[4..], &mut output[4..], 2));
    assert_same_bits(&output[4..], &input[..4]);
}

#[test]
fn zero_delay_is_bitwise_identity_and_leaves_longer_suffixes_untouched() {
    let input = [
        f32::from_bits(0x8000_0000),
        f32::from_bits(0x7fc1_2345),
        f32::from_bits(0x0000_0001),
        f32::from_bits(0xffc5_4321),
        6.0,
    ];
    let mut output = [f32::from_bits(0x4120_0000); 6];
    let suffix = output[4..].to_vec();
    let mut line = RouteDelayLine::new(2, 0).expect("valid zero-delay line");

    assert_eq!(line.history_samples(), 0);
    assert!(line.advance(&[], &mut [], 0));
    assert!(line.advance(&input, &mut output, 2));

    assert_same_bits(&output[..4], &input[..4]);
    assert_same_bits(&output[4..], &suffix);
}

#[test]
fn reset_silences_stale_history_without_clearing_the_line() {
    let mut line = RouteDelayLine::new(1, 3).expect("valid delay line");
    let old_input = [1.0, 2.0, 3.0, 4.0];
    let mut old_output = [0.0; 4];
    assert!(line.advance(&old_input, &mut old_output, old_input.len()));
    assert_same_bits(&old_output, &[0.0, 0.0, 0.0, 1.0]);

    line.reset();
    let new_input = [9.0, 10.0, 11.0, 12.0, 13.0];
    let mut new_output = [-1.0; 5];
    assert!(line.advance(&new_input, &mut new_output, new_input.len()));

    assert_same_bits(&new_output, &[0.0, 0.0, 0.0, 9.0, 10.0]);
}

#[test]
fn rejected_spans_and_overflow_leave_output_and_history_unchanged() {
    let mut line = RouteDelayLine::new(2, 2).expect("valid delay line");
    let mut initial_output = [f32::from_bits(0xc120_0000); 4];
    assert!(line.advance(&[1.0, 1.0, 2.0, 2.0], &mut initial_output, 2));
    assert!(line.advance(&[3.0, 3.0], &mut [0.0; 2], 1));
    assert_same_bits(&initial_output, &[0.0; 4]);

    let sentinel = [f32::from_bits(0x4120_0000); 4];
    let mut output = sentinel;
    assert!(!line.advance(&[4.0, 4.0, 5.0], &mut output, 2));
    assert_same_bits(&output, &sentinel);

    assert!(!line.advance(&[4.0; 4], &mut output[..3], 2));
    assert_same_bits(&output, &sentinel);

    assert!(!line.advance(&[], &mut [], usize::MAX));
    assert_same_bits(&output, &sentinel);

    let mut next = [0.0; 2];
    assert!(line.advance(&[4.0, 4.0], &mut next, 1));
    assert_same_bits(&next, &[2.0, 2.0]);
}

#[test]
fn constructor_enforces_channel_and_history_bounds() {
    assert!(matches!(
        RouteDelayLine::new(0, 1),
        Err(RouteDelayError::InvalidChannels { channels: 0 })
    ));
    assert!(matches!(
        RouteDelayLine::new(65, 1),
        Err(RouteDelayError::InvalidChannels { channels: 65 })
    ));
    assert!(matches!(
        RouteDelayLine::new(64, usize::MAX),
        Err(RouteDelayError::CapacityOverflow)
    ));

    let channels = 64;
    let maximum_frames = MAX_HISTORY_BYTES / std::mem::size_of::<f32>() / channels;
    let maximum_line =
        RouteDelayLine::new(channels, maximum_frames).expect("exactly bounded history");
    assert_eq!(maximum_line.history_samples(), maximum_frames * channels);

    let requested_bytes = (maximum_frames + 1) * channels * std::mem::size_of::<f32>();
    assert!(matches!(
        RouteDelayLine::new(channels, maximum_frames + 1),
        Err(RouteDelayError::HistoryTooLarge {
            requested_bytes: actual_bytes,
            maximum_bytes: MAX_HISTORY_BYTES,
        }) if actual_bytes == requested_bytes
    ));
}
