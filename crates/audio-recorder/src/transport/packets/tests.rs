use super::{FREE, PACKETS, Packet, PacketRing, READING, WRITING};
use crate::transport::{RecorderTap, Transport};
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

#[test]
fn failed_pcm_preparation_reclaims_partial_slots_and_preserves_active_recording() {
    let active = Transport::new(1, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&active));
    active.gate.store(1, Ordering::SeqCst);
    let mut packet = Packet::new(1).unwrap();
    for failure in [0, PACKETS / 2, PACKETS - 1] {
        tap.capture(&[0.25; 256], 48_000);
        let mut attempts = 0;
        let allocation = allocation_counter::measure(|| {
            let result = PacketRing::prepare(2, |count| {
                let current = attempts;
                attempts += 1;
                if current == failure {
                    return Err(mixrelay_audio_io::pcm_buffer::zeroed(usize::MAX).unwrap_err());
                }
                super::allocate_samples(count)
            });
            assert!(result.is_err());
        });
        assert_eq!(attempts, failure + 1);
        assert_eq!((allocation.count_current, allocation.bytes_current), (0, 0));
        assert!(active.read_packet(&mut packet));
        assert_eq!(packet.frames, 256);
        assert!(
            packet
                .samples
                .iter()
                .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
        );
    }
    assert!(Packet::new(usize::MAX).is_err());
    assert!(PacketRing::new(usize::MAX).is_err());
}

#[test]
fn preempted_worker_slot_never_makes_capture_wait_or_touch_its_pcm() {
    let shared = Transport::new(2, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&shared));
    shared.gate.store(1, Ordering::SeqCst);
    let slot = &shared.packets.slots[0];
    slot.state.store(READING, Ordering::Release);
    slot.samples[0].store(0.75_f32.to_bits(), Ordering::Relaxed);
    // The worker remains preempted throughout the capture call. Waiting for it
    // would deadlock this fixture; returning an explicit fault is the contract.
    let allocation = allocation_counter::measure(|| tap.capture(&[0.25; 512], 48_000));
    assert_eq!((allocation.count_total, allocation.count_current), (0, 0));
    assert_eq!(slot.state.load(Ordering::Acquire), READING);
    assert_eq!(slot.samples[0].load(Ordering::Relaxed), 0.75_f32.to_bits());
    assert_eq!(shared.cutoff(), Some(crate::AdmissionFailure::Overrun));
    slot.state.store(FREE, Ordering::Release);
}

#[test]
fn consumer_does_not_wait_for_an_unpublished_packet() {
    let ring = PacketRing::new(1).unwrap();
    let mut packet = Packet::new(1).unwrap();
    ring.slots[0].state.store(WRITING, Ordering::Release);
    assert!(!ring.read(&mut packet));
    assert_eq!(ring.read.load(Ordering::Relaxed), 0);
    assert_eq!(packet.frames, 0);
}

#[test]
fn packet_pcm_and_rate_metadata_remain_fifo_across_concurrent_ring_wraps() {
    let ring = Arc::new(PacketRing::new(2).unwrap());
    let producer_ring = Arc::clone(&ring);
    let producer = std::thread::spawn(move || {
        for index in 0..4096_u16 {
            let rate = if index % 2 == 0 { 44_100 } else { 48_000 };
            let value = f32::from(index);
            let stalled = Instant::now();
            while !producer_ring.publish(&[value, -value], rate) {
                assert!(
                    stalled.elapsed() < Duration::from_secs(5),
                    "consumer stopped before packet {index}"
                );
                std::thread::yield_now();
            }
        }
        producer_ring.finish_pending();
    });
    let mut packet = Packet::new(2).unwrap();
    for index in 0..4096_u16 {
        let stalled = Instant::now();
        while !ring.read(&mut packet) {
            assert!(
                stalled.elapsed() < Duration::from_secs(5),
                "producer stopped before packet {index}"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            (packet.frames, packet.rate),
            (1, if index % 2 == 0 { 44_100 } else { 48_000 })
        );
        assert_eq!(packet.samples[0].to_bits(), f32::from(index).to_bits());
        assert_eq!(packet.samples[1].to_bits(), (-f32::from(index)).to_bits());
    }
    producer.join().unwrap();
    assert!(!ring.read(&mut packet));
    assert_eq!(ring.slots.len(), PACKETS);
}

#[test]
fn concurrent_tap_clone_and_retirement_do_not_change_packet_ownership() {
    let shared = Transport::new(1, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&shared));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let tap = &tap;
            scope.spawn(move || {
                for _ in 0..1024 {
                    drop(tap.clone());
                }
            });
        }
    });
    assert_eq!(shared.taps.load(Ordering::SeqCst), 1);
    shared.gate.store(1, Ordering::SeqCst);
    tap.capture(&[0.5], 48_000);
    assert_eq!(shared.cutoff(), None);
    let mut packet = Packet::new(1).unwrap();
    assert!(shared.read_packet(&mut packet));
    assert_eq!(packet.samples[0].to_bits(), 0.5_f32.to_bits());
    drop(tap);
    assert_eq!(shared.taps.load(Ordering::SeqCst), 0);
}

#[test]
fn small_callbacks_use_the_full_fixed_frame_capacity_without_allocation() {
    let shared = Transport::new(8, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&shared));
    shared.gate.store(1, Ordering::SeqCst);
    let allocation = allocation_counter::measure(|| {
        for _ in 0..PACKETS * 8 {
            tap.capture(&[0.25; 32 * 8], 48_000);
        }
    });
    assert_eq!((allocation.count_total, allocation.count_current), (0, 0));
    assert_eq!(shared.gate.load(Ordering::SeqCst), 1);
    tap.capture(&[0.5; 8], 48_000);
    assert_eq!(shared.cutoff(), Some(crate::AdmissionFailure::Overrun));
    let mut packet = Packet::new(8).unwrap();
    for _ in 0..PACKETS {
        assert!(shared.read_packet(&mut packet));
        assert_eq!((packet.frames, packet.rate), (256, 48_000));
        assert!(
            packet
                .samples
                .iter()
                .all(|value| value.to_bits() == 0.25_f32.to_bits())
        );
    }
    assert!(!shared.read_packet(&mut packet));
}

#[test]
fn partial_rate_change_and_cutoff_preserve_fifo_and_each_original_rate() {
    let shared = Transport::new(2, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&shared));
    shared.gate.store(1, Ordering::SeqCst);
    tap.capture(&[0.25; 64], 48_000);
    let mut packet = Packet::new(2).unwrap();
    assert!(!shared.read_packet(&mut packet));
    tap.capture(&[0.5; 96], 44_100);
    assert!(shared.read_packet(&mut packet));
    assert_eq!((packet.frames, packet.rate), (32, 48_000));
    assert!(
        packet.samples[..64]
            .iter()
            .all(|value| value.to_bits() == 0.25_f32.to_bits())
    );
    assert!(!shared.read_packet(&mut packet));
    assert_eq!(shared.cutoff(), None);
    assert!(shared.read_packet(&mut packet));
    assert_eq!((packet.frames, packet.rate), (48, 44_100));
    assert!(
        packet.samples[..96]
            .iter()
            .all(|value| value.to_bits() == 0.5_f32.to_bits())
    );
    assert!(!shared.read_packet(&mut packet));
}

#[test]
fn carried_partial_plus_maximum_callback_keeps_all_frames_in_order() {
    let shared = Transport::new(1, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&shared));
    shared.gate.store(1, Ordering::SeqCst);
    tap.capture(&[0.25], 48_000);
    let allocation = allocation_counter::measure(|| tap.capture(&[0.5; 4096], 48_000));
    assert_eq!((allocation.count_total, allocation.count_current), (0, 0));
    assert_eq!(shared.cutoff(), None);
    let mut packet = Packet::new(1).unwrap();
    let mut frames = 0;
    for _ in 0..PACKETS {
        if !shared.read_packet(&mut packet) {
            break;
        }
        for sample in &packet.samples[..packet.frames] {
            assert_eq!(
                sample.to_bits(),
                if frames == 0 { 0.25_f32 } else { 0.5_f32 }.to_bits()
            );
            frames += 1;
        }
    }
    assert!(!shared.read_packet(&mut packet));
    assert_eq!(frames, 4097);
}
