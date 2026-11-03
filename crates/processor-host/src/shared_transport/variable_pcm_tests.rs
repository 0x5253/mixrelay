use super::memory::endpoints;
use crate::AudioLayout;

const QUANTUM: usize = 128;

fn sample(frame: usize, channel: usize) -> f32 {
    f32::from(u16::try_from((frame % 512) * 64 + channel + 1).unwrap())
}

fn verify_partitions(channels: usize, partitions: &[usize]) {
    let layout = AudioLayout {
        inputs: channels,
        outputs: channels,
        block_frames: QUANTUM,
    };
    let (mut parent, _, mut worker, _) =
        endpoints(layout, crate::instance_id::InstanceId::new(1).unwrap());
    let mut input = vec![0.0; QUANTUM * channels];
    let mut output = vec![0.0; input.len()];
    let mut processed = vec![0.0; input.len()];
    let mut position = 0;
    let mut dsp_frames = 0;
    let allocations = allocation_counter::measure(|| {
        for (index, frames) in partitions.iter().copied().cycle().take(96).enumerate() {
            let samples = frames * channels;
            let ready = parent.receive(&mut output[..samples], frames, |_| {});
            assert!(
                ready,
                "startup silence and delayed PCM must both be renderable"
            );
            if ready {
                for (offset, actual) in output[..samples].iter().enumerate() {
                    let expected = (position + offset / channels)
                        .checked_sub(QUANTUM)
                        .map_or(0.0, |frame| sample(frame, offset % channels));
                    assert_eq!(
                        actual.to_bits(),
                        expected.to_bits(),
                        "callback={index}, sample={offset}"
                    );
                }
            }
            for (offset, value) in input[..samples].iter_mut().enumerate() {
                *value = sample(position + offset / channels, offset % channels);
            }
            assert!(parent.submit(
                u64::try_from(index + 1).unwrap(),
                &input[..samples],
                frames,
                || {}
            ));
            let actual_frames = worker.receive(|source| {
                assert_eq!(source.len(), samples);
                processed[..samples].copy_from_slice(source);
                dsp_frames += source.len() / channels;
            });
            assert_eq!(actual_frames, Some(frames));
            assert!(worker.publish(|destination| {
                assert_eq!(destination.len(), samples);
                destination.copy_from_slice(&processed[..samples]);
            }));
            position += frames;
        }
    });
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(dsp_frames, position, "DSP must not advance through padding");
}

#[test]
fn uneven_pcm_packets_preserve_every_channel_without_callback_allocation() {
    for channels in [1, 2, 32, 64] {
        verify_partitions(channels, &[16, 32, 48, 32, 112, 16, 127, 1, 64, 64, 80, 48]);
        verify_partitions(channels, &[17, 127, 3, 65, 119, 1, 128, 7]);
    }
}

#[test]
fn full_quantum_pcm_keeps_the_existing_one_block_delay_without_callback_allocation() {
    for channels in [2, 32, 64] {
        verify_partitions(channels, &[128]);
    }
}

#[test]
fn suspension_discards_published_and_in_flight_responses_without_rewinding_time() {
    for publish_before_suspend in [false, true] {
        for channels in [1, 2, 32, 64] {
            let layout = AudioLayout {
                inputs: channels,
                outputs: channels,
                block_frames: 4,
            };

            let (mut parent, _, mut worker, _) =
                endpoints(layout, crate::instance_id::InstanceId::new(1).unwrap());
            let old = vec![7.0; 4 * channels];
            let fresh = vec![2.0; 4 * channels];
            let mut output = vec![9.0; 4 * channels];
            assert!(parent.submit(1, &old, 4, || {}));
            assert_eq!(worker.receive(|_| {}), Some(4));
            if publish_before_suspend {
                assert!(worker.publish(|pcm| pcm.fill(7.0)));
            }
            let allocations = allocation_counter::measure(|| {
                for _ in 0..100 {
                    parent.discard_history();
                }
                if !publish_before_suspend {
                    assert!(worker.publish(|pcm| pcm.fill(7.0)));
                }
                assert!(parent.receive(&mut output, 4, |_| {}));
                assert!(output.iter().all(|&sample| sample == 0.0));
                assert_eq!(parent.acknowledged_output(), 1);
                assert!(parent.submit(2, &fresh, 4, || {}));
                assert_eq!(worker.receive(|pcm| assert_eq!(pcm, fresh)), Some(4));
                assert!(worker.publish(|pcm| pcm.fill(2.0)));
                assert!(parent.receive(&mut output, 4, |_| {}));
                assert_eq!(output, fresh);
            });
            assert_eq!(allocations.count_total, 0);
        }
    }
}

#[test]
fn suspension_invalidates_cached_suffix_and_retains_variable_frame_alignment() {
    let layout = AudioLayout {
        inputs: 1,
        outputs: 1,
        block_frames: 4,
    };

    let (mut parent, _, mut worker, _) =
        endpoints(layout, crate::instance_id::InstanceId::new(1).unwrap());
    let mut output = [9.0; 4];
    assert!(parent.submit(1, &[7.0; 4], 4, || {}));
    assert_eq!(worker.receive(|_| {}), Some(4));
    assert!(worker.publish(|pcm| pcm.fill(7.0)));
    assert!(parent.receive(&mut output[..2], 2, |_| {}));
    assert_eq!(output[..2], [7.0; 2]);
    parent.discard_history();
    for (sequence, frames, value) in [(2, 3, 2.0), (3, 2, 3.0)] {
        assert!(parent.receive(&mut output[..frames], frames, |_| {}));
        let expected = if sequence == 2 {
            [0.0, 0.0, 0.0]
        } else {
            [0.0, 2.0, 0.0]
        };
        assert_eq!(output[..frames], expected[..frames]);
        assert!(parent.submit(sequence, &[value; 4], frames, || {}));
        assert_eq!(worker.receive(|_| {}), Some(frames));
        assert!(worker.publish(|pcm| pcm.fill(value)));
    }
    assert!(parent.receive(&mut output, 4, |_| {}));
    assert_eq!(
        output.map(f32::to_bits),
        [2.0_f32, 2.0, 3.0, 3.0].map(f32::to_bits)
    );
}

#[test]
fn duplicate_or_invalid_sequence_does_not_consume_callback_time() {
    let layout = AudioLayout {
        inputs: 1,
        outputs: 1,
        block_frames: 4,
    };
    let (mut parent, _, mut worker, _) =
        endpoints(layout, crate::instance_id::InstanceId::new(1).unwrap());
    assert!(parent.submit(1, &[1.0; 4], 4, || {}));
    for sequence in [0, 1, u64::MAX] {
        assert!(!parent.submit(sequence, &[9.0; 4], 4, || panic!("invalid publication")));
    }
    assert_eq!(worker.receive(|_| {}), Some(4));
    assert!(worker.publish(|output| output.fill(1.0)));
    let mut output = [0.0; 4];
    assert!(parent.receive(&mut output, 4, |_| {}));
    assert_eq!(output.map(f32::to_bits), [1.0_f32; 4].map(f32::to_bits));
    assert!(parent.submit(2, &[2.0; 4], 4, || {}));
    assert_eq!(worker.receive(|_| {}), Some(4));
    assert!(worker.publish(|output| output.fill(2.0)));
    assert!(parent.receive(&mut output, 4, |_| {}));
    assert_eq!(output.map(f32::to_bits), [2.0_f32; 4].map(f32::to_bits));
}

#[test]
fn malformed_input_packet_is_acknowledged_without_copying_or_publishing() {
    use std::sync::atomic::Ordering;
    for (frames, first_frame) in [(0, 0), (129, 0), (1, u64::MAX)] {
        let layout = AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: QUANTUM,
        };
        let (mut parent, _, mut worker, _) =
            endpoints(layout, crate::instance_id::InstanceId::new(1).unwrap());
        assert!(parent.submit(1, &[0.0; 256], QUANTUM, || {}));
        parent
            .header()
            .input_frames
            .store(frames, Ordering::Relaxed);
        parent
            .header()
            .input_first_frame
            .store(first_frame, Ordering::Relaxed);
        assert_eq!(worker.receive(|_| panic!("invalid packet copied")), None);
        assert!(!worker.publish(|_| panic!("invalid output reserved")));
        assert_eq!(parent.header().input_consumed.load(Ordering::Acquire), 1);
        assert!(parent.submit(2, &[0.0; 256], QUANTUM, || {}));
        assert_eq!(
            worker.receive(|source| assert_eq!(source.len(), 256)),
            Some(QUANTUM)
        );
    }
}
