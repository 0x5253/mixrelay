use super::{CLOSED, Flight, PACKETS, Packet, Producer, RecorderTap, Transport};
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

mod retention;

#[test]
fn cutoff_waits_for_admitted_packet_and_observes_its_late_failure() {
    for late_failure in [false, true] {
        let shared = Transport::new(1, std::thread::yield_now).unwrap();
        shared.gate.store(1, Ordering::SeqCst);
        shared.producer.store(true, Ordering::SeqCst);
        shared.in_flight.store(1, Ordering::SeqCst);
        let flight = Flight(&shared.in_flight);
        let producer = Producer(&shared.producer);
        let other = Arc::clone(&shared);
        let control = std::thread::spawn(move || other.cutoff());
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.gate.load(Ordering::SeqCst) & CLOSED == 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(!control.is_finished());
        assert!(shared.packets.publish(&[0.25], 48_000));
        if late_failure {
            shared.fail(1, 1);
        }
        drop(producer);
        drop(flight);
        assert_eq!(control.join().unwrap().is_some(), late_failure);
        let mut packet = Packet::new(1).unwrap();
        assert!(shared.read_packet(&mut packet));
        assert_eq!(
            (packet.frames, packet.samples[0].to_bits()),
            (1, 0.25_f32.to_bits())
        );
    }
}

#[test]
fn a_full_packet_pool_fails_without_allocating_freeing_or_retagging() {
    let shared = Transport::new(1, std::thread::yield_now).unwrap();
    let tap = RecorderTap::new(Arc::clone(&shared));
    shared.gate.store(1, Ordering::SeqCst);
    for _ in 0..PACKETS {
        tap.capture(&[0.5; 256], 48_000);
    }
    let allocations = allocation_counter::measure(|| tap.capture(&[0.5; 256], 48_000));
    assert_eq!((allocations.count_total, allocations.count_current), (0, 0));
    assert_eq!(shared.cutoff(), Some(crate::AdmissionFailure::Overrun));
    shared.gate.store(2, Ordering::SeqCst);
    shared.fail(1, 2);
    assert_eq!(shared.gate.load(Ordering::SeqCst), 2);
}
