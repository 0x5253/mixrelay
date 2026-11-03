use super::*;
use std::thread;

#[test]
fn geometry_is_fixed_before_any_role_is_published() {
    for channels in [0, 65, usize::MAX] {
        assert!(matches!(pcm_fifo(channels, 4), Err(PcmFifoError::Channels)));
    }
    for capacity in [0, 65537, usize::MAX] {
        assert!(matches!(pcm_fifo(2, capacity), Err(PcmFifoError::Capacity)));
    }
    let (producer, consumer) = pcm_fifo(64, 1).unwrap();
    assert_eq!(producer.write(&[0.25; 64]), 1);
    assert_eq!(producer.stats().queued_frames, 1);
    assert_eq!(consumer.read(&mut [0.0; 64]), 1);
}

#[test]
fn partial_frames_are_never_transferred_and_short_reads_silence_the_suffix() {
    for channels in [1, 2, 8, 32, 64] {
        let (producer, consumer) = pcm_fifo(channels, 5).unwrap();
        let source = vec![0.25; channels * 2 + channels - 1];
        assert_eq!(producer.write(&source), 2);
        let mut output = vec![9.0; channels * 3 + channels - 1];
        assert_eq!(consumer.read(&mut output), 2);
        assert!(
            output[..channels * 2]
                .iter()
                .all(|s| s.to_bits() == 0.25_f32.to_bits())
        );
        assert!(output[channels * 2..].iter().all(|s| s.to_bits() == 0));
        assert_eq!(consumer.stats().consumed_frames, 2);
    }
}

#[test]
fn overflow_and_non_power_of_two_wrap_preserve_unread_frames() {
    let (producer, consumer) = pcm_fifo(2, 3).unwrap();
    for _ in 0..1000 {
        assert_eq!(producer.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 3);
        assert_eq!(producer.write(&[7.0, 8.0]), 0);
        let mut first = [0.0; 2];
        assert_eq!(consumer.read(&mut first), 1);
        assert_eq!(first.map(f32::to_bits), [1.0_f32, 2.0].map(f32::to_bits));
        assert_eq!(producer.write(&[7.0, 8.0]), 1);
        let mut tail = [0.0; 6];
        assert_eq!(consumer.read(&mut tail), 3);
        assert_eq!(
            tail.map(f32::to_bits),
            [3.0_f32, 4.0, 5.0, 6.0, 7.0, 8.0].map(f32::to_bits)
        );
    }
}

#[test]
fn discard_is_not_consumer_clock_work_and_does_not_erase_later_publication() {
    let (producer, consumer) = pcm_fifo(1, 8).unwrap();
    assert_eq!(producer.write(&[1.0; 4]), 4);
    assert_eq!(consumer.read(&mut [0.0; 2]), 2);
    assert!(consumer.discard());
    assert_eq!(consumer.stats().queued_frames, 0);
    assert_eq!(producer.stats().consumed_frames, 2);
    assert_eq!(producer.write(&[2.0; 3]), 3);
    let mut output = [0.0; 3];
    assert_eq!(consumer.read(&mut output), 3);
    assert_eq!(output.map(f32::to_bits), [2.0_f32; 3].map(f32::to_bits));
    assert_eq!(consumer.stats().written_frames, 7);
    assert_eq!(consumer.stats().consumed_frames, 5);
}

#[test]
fn overlapping_operations_fail_without_waiting_or_moving_positions() {
    let (producer, consumer) = pcm_fifo(1, 8).unwrap();
    let writer = Operation::begin(&producer.busy).unwrap();
    assert_eq!(producer.write(&[1.0]), 0);
    assert_eq!(producer.stats().written_frames, 0);
    drop(writer);
    assert_eq!(producer.write(&[1.0]), 1);
    let reader = Operation::begin(&consumer.busy).unwrap();
    let mut output = [9.0];
    assert_eq!(consumer.read(&mut output), 0);
    assert_eq!(output[0].to_bits(), 0);
    assert!(!consumer.discard());
    assert_eq!(consumer.stats().queued_frames, 1);
    drop(reader);
    assert_eq!(consumer.read(&mut output), 1);
    assert_eq!(output[0].to_bits(), 1.0_f32.to_bits());
}

#[test]
fn clock_counter_wrap_cannot_change_non_power_of_two_storage_positions() {
    let (producer, consumer) = pcm_fifo(2, 3).unwrap();
    producer
        .shared
        .written
        .store(u64::MAX - 1, Ordering::Relaxed);
    consumer
        .shared
        .consumed
        .store(u64::MAX - 1, Ordering::Relaxed);
    let source = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    assert_eq!(producer.write(&source), 3);
    let mut output = [0.0; 6];
    assert_eq!(consumer.read(&mut output), 3);
    assert_eq!(output.map(f32::to_bits), source.map(f32::to_bits));
    assert_eq!(producer.stats().written_frames, 1);
    assert_eq!(consumer.stats().consumed_frames, 1);
}

#[test]
fn separate_thread_roles_keep_32_and_64_channel_pcm_bit_exact() {
    for channels in [32, 64] {
        let (producer, consumer) = pcm_fifo(channels, 7).unwrap();
        let writer = thread::spawn(move || {
            let mut frame = vec![0.0; channels];
            for sequence in 1..=10_000_u32 {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = f32::from_bits(sequence * 64 + u32::try_from(channel).unwrap());
                }
                while producer.write(&frame) == 0 {
                    thread::yield_now();
                }
            }
        });
        let mut frame = vec![0.0; channels];
        for sequence in 1..=10_000_u32 {
            while consumer.read(&mut frame) == 0 {
                thread::yield_now();
            }
            for (channel, sample) in frame.iter().enumerate() {
                assert_eq!(
                    sample.to_bits(),
                    sequence * 64 + u32::try_from(channel).unwrap()
                );
            }
        }
        writer.join().unwrap();
    }
}

#[test]
fn storage_lives_until_both_roles_are_released() {
    let (producer, consumer) = pcm_fifo(2, 8).unwrap();
    let storage = Arc::downgrade(&producer.shared);
    drop(producer);
    assert!(storage.upgrade().is_some());
    drop(consumer);
    assert!(storage.upgrade().is_none());
}

#[test]
fn debug_describes_geometry_without_exposing_pcm_storage() {
    let (producer, consumer) = pcm_fifo(32, 4096).unwrap();
    assert_eq!(producer.write(&[0.25; 64]), 2);
    for text in [format!("{producer:?}"), format!("{consumer:?}")] {
        assert!(text.contains("channels: 32"));
        assert!(text.contains("queued_frames: 2"));
        assert!(!text.contains("samples"));
        assert!(text.len() < 300);
    }
}
