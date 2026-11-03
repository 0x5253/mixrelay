//! Exact source coordinates remain attached across partial graph reads.
use super::*;
use std::{ptr, time::Duration};

#[test]
fn acquired_pcm_age_weights_partial_reads_and_rejects_unknown_or_future_times() {
    let io = DeviceIo::for_test(c"unit", true, 1, 48_000).unwrap();
    let pointer = ptr::from_ref(&io).cast_mut();
    let mut samples = [0.0; 8];
    let tick = |nanos| io.timebase.ticks(Duration::from_nanos(nanos));
    let read = |samples: &mut [f32], label| {
        // SAFETY: unpublished owner; this test is the only serialized graph consumer.
        unsafe {
            crate::device_io::queues::graph_read(
                &io,
                samples.as_mut_ptr(),
                u32::try_from(samples.len()).unwrap(),
                1,
                (label, None),
            )
        }
    };
    assert!(read(&mut samples[..1], tick(1_000_000_000)).initial_epoch);
    // SAFETY: the initialized owner remains on this stack throughout the snapshot.
    let before = unsafe { latency_snapshot(pointer, false) }.unwrap();
    let times = std::array::from_fn::<_, 8, _>(|frame| 1_000_000_000 + frame as u64 * 20_000);
    // SAFETY: unpublished owner, sole producer, complete disjoint PCM/time spans.
    assert_eq!(
        unsafe { crate::device_io::bridge_write_timed(pointer, &[0.25; 8], &times) },
        8
    );
    let measured = allocation_counter::measure(|| {
        assert_eq!(read(&mut samples[..3], tick(2_000_000_000)).frames, 3);
        assert_eq!(read(&mut samples[3..], tick(3_000_000_000)).frames, 5);
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(samples.map(f32::to_bits), [0.25_f32.to_bits(); 8]);
    // SAFETY: the same retained owner; the reader only observes atomic taps.
    let after = unsafe { latency_snapshot(pointer, false) }.unwrap();
    let age = |label, at: &u64| {
        f64::from(u32::try_from(io.timebase.nanoseconds(tick(label)) - at).unwrap())
    };
    let expected = (times[..3]
        .iter()
        .map(|at| age(2_000_000_000, at))
        .sum::<f64>()
        + times[3..]
            .iter()
            .map(|at| age(3_000_000_000, at))
            .sum::<f64>())
        / 8.0;
    assert!((after.content_age.mean_since(before.content_age).unwrap() - expected).abs() <= 1.0);
    for acquired in [0, 5_000_000_000] {
        // SAFETY: the owner is retained and these snapshots read only atomic taps.
        let before = unsafe { latency_snapshot(pointer, false) }.unwrap();
        // SAFETY: the previous consumer drained all eight frames; the same sole roles continue.
        assert_eq!(
            unsafe { crate::device_io::bridge_write_timed(pointer, &[0.5; 8], &[acquired; 8]) },
            8
        );
        assert_eq!(read(&mut samples, tick(4_000_000_000)).frames, 8);
        // SAFETY: the same live owner after the serialized consumer completes.
        let after = unsafe { latency_snapshot(pointer, false) }.unwrap();
        assert_eq!(after.content_age.mean_since(before.content_age), None);
    }
}
