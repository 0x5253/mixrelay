use super::*;

fn event(count: u64) -> ClockBreak {
    let low = u32::try_from(count & u64::from(u32::MAX)).unwrap();
    let previous_host = count.wrapping_mul(10);
    ClockBreak {
        count,
        expected_sample: f64::from(low) + 0.5,
        actual_sample: f64::from(low) + 64.5,
        previous_host,
        actual_host: previous_host.wrapping_add(2),
        flags: count ^ 0x53,
        callback_host: previous_host.wrapping_add(3),
        callback_flags: count ^ 0x37,
        previous_enter: previous_host.wrapping_add(4),
        previous_exit: previous_host.wrapping_add(5),
        callback_enter: previous_host.wrapping_add(6),
    }
}

fn assert_event(actual: ClockBreak, count: u64) {
    let expected = event(count);
    assert_eq!(actual.count, expected.count);
    assert_eq!(
        actual.expected_sample.to_bits(),
        expected.expected_sample.to_bits()
    );
    assert_eq!(
        actual.actual_sample.to_bits(),
        expected.actual_sample.to_bits()
    );
    assert_eq!(
        (
            actual.previous_host,
            actual.actual_host,
            actual.flags,
            actual.callback_host,
            actual.callback_flags
        ),
        (
            expected.previous_host,
            expected.actual_host,
            expected.flags,
            expected.callback_host,
            expected.callback_flags
        )
    );
    assert_eq!(
        (
            actual.previous_enter,
            actual.previous_exit,
            actual.callback_enter
        ),
        (
            expected.previous_enter,
            expected.previous_exit,
            expected.callback_enter
        )
    );
}

#[test]
fn burst_history_is_ordered_and_readers_have_independent_cursors() {
    let evidence = ClockEvidence::default();
    assert!(evidence.read(0).is_none());
    for count in 1..=10 {
        evidence.publish(event(count));
    }
    for _reader in 0..2 {
        let mut cursor = 0;
        for count in 1..=10 {
            let record = evidence.read(cursor).unwrap();
            assert_event(record.event, count);
            assert_eq!(record.missed_before, 0);
            cursor = record.next_cursor;
        }
        assert!(evidence.read(cursor).is_none());
    }
}

#[test]
fn slow_reader_accounts_for_every_overwrite_without_blocking_publication() {
    let evidence = ClockEvidence::default();
    let total = u64::try_from(CLOCK_BREAK_CAPACITY).unwrap() * 3 + 7;
    for count in 1..=total {
        evidence.publish(event(count));
    }
    let mut cursor = 0;
    let (mut received, mut missed) = (0, 0);
    while let Some(record) = evidence.read(cursor) {
        assert_event(record.event, cursor + record.missed_before + 1);
        missed += record.missed_before;
        received += 1;
        cursor = record.next_cursor;
    }
    assert_eq!(received, u64::try_from(CLOCK_BREAK_CAPACITY).unwrap());
    assert_eq!(received + missed, total);
    assert_eq!(cursor, total);
    evidence.publish(event(total + 1));
    assert_eq!(evidence.read(cursor).unwrap().missed_before, 0);
}

#[test]
fn cursor_and_native_counter_wrap_do_not_turn_real_events_into_empty_slots() {
    let evidence = ClockEvidence::default();
    let mut cursor = u64::MAX - 1;
    evidence.published.store(cursor, SeqCst);
    for count in [u64::MAX, 0, 1] {
        evidence.publish(event(count));
    }
    for count in [u64::MAX, 0, 1] {
        let record = evidence.read(cursor).unwrap();
        assert_event(record.event, count);
        assert_eq!(record.missed_before, 0);
        cursor = record.next_cursor;
    }
    assert_eq!(cursor, 1);
    assert!(evidence.read(cursor).is_none());
}

#[test]
fn pending_or_overwritten_slot_and_future_cursor_never_fabricate_a_record() {
    let evidence = ClockEvidence::default();
    evidence.publish(event(2));
    assert!(evidence.read(2).is_none());
    let slot = evidence.slot(0);
    slot.sequence.fetch_add(1, SeqCst);
    assert!(evidence.read(0).is_none());
    slot.sequence.fetch_add(1, SeqCst);
    assert_event(evidence.read(0).unwrap().event, 2);
    slot.position.store(64, SeqCst);
    assert!(evidence.read(0).is_none());
}

#[test]
fn concurrent_publication_has_coherent_payloads_and_complete_loss_accounting() {
    use std::{sync::Arc, thread};
    const TOTAL: u64 = 10_000;
    let evidence = Arc::new(ClockEvidence::default());
    let writer = Arc::clone(&evidence);
    let producer = thread::spawn(move || {
        for count in 1..=TOTAL {
            writer.publish(event(count));
            if count % 3 == 0 {
                thread::yield_now();
            }
        }
    });
    let (mut cursor, mut received, mut missed) = (0, 0, 0);
    let mut read_next = || {
        let Some(record) = evidence.read(cursor) else {
            return false;
        };
        assert_event(record.event, cursor + record.missed_before + 1);
        cursor = record.next_cursor;
        received += 1;
        missed += record.missed_before;
        true
    };
    // Exercise concurrent reads for a finite workload; scheduling speed is not
    // the journal's contract. Joining observes producer failure and completion.
    for _ in 0..TOTAL {
        if !read_next() {
            thread::yield_now();
        }
    }
    producer.join().unwrap();
    // A completed writer can leave at most one ring of unread publications.
    for _ in 0..CLOCK_BREAK_CAPACITY {
        if !read_next() {
            break;
        }
    }
    assert_eq!(cursor, TOTAL);
    assert_eq!(received + missed, TOTAL);
    assert!(evidence.read(cursor).is_none());
}

#[test]
fn ffi_failure_leaves_output_untouched_and_readers_do_not_consume_publications() {
    let io = DeviceIo::for_test(c"clock-journal-fixture", false, 2, 48_000).unwrap();
    let mut record = ClockBreakRecord {
        next_cursor: 99,
        missed_before: 88,
        event: event(77),
    };
    for pointer in [std::ptr::null(), std::ptr::from_ref(&io)] {
        // SAFETY: null or retained fixture; complete exclusive, disjoint output.
        assert!(!unsafe { mr_device_io_next_clock_break(pointer, 0, &raw mut record) });
        assert_eq!((record.next_cursor, record.missed_before), (99, 88));
        assert_event(record.event, 77);
    }
    io.clock_evidence.publish(event(1));
    // SAFETY: retained owner, null output is a specified failure path.
    assert!(!unsafe { mr_device_io_next_clock_break(&raw const io, 0, std::ptr::null_mut()) });
    // SAFETY: fixture remains alive with a complete exclusive output.
    assert!(unsafe { mr_device_io_next_clock_break(&raw const io, 0, &raw mut record) });
    assert_event(record.event, 1);
    assert_eq!((record.next_cursor, record.missed_before), (1, 0));
    // SAFETY: same retained fixture, a second cursor reads without consuming the first.
    assert!(unsafe { mr_device_io_next_clock_break(&raw const io, 0, &raw mut record) });
    assert_event(record.event, 1);
}

#[test]
#[ignore = "serial release clock-evidence publication measurement, no native devices"]
fn measure_clock_evidence_publication() {
    use std::{hint::black_box, time::Instant};
    const ITERATIONS: u32 = 200_000;
    for run in 0..6 {
        let evidence = ClockEvidence::default();
        let event = ClockBreak {
            count: 1,
            expected_sample: 320.5,
            actual_sample: 384.5,
            previous_host: 10,
            actual_host: 12,
            flags: 3,
            callback_host: 13,
            callback_flags: 2,
            previous_enter: 1,
            previous_exit: 2,
            callback_enter: 3,
        };
        for _ in 0..10_000 {
            evidence.publish(black_box(event));
        }
        let allocation = allocation_counter::measure(|| {
            for _ in 0..1000 {
                evidence.publish(black_box(event));
            }
        });
        assert_eq!(allocation.count_total, 0);
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            evidence.publish(black_box(event));
        }
        let nanos = start.elapsed().as_nanos();
        black_box(&evidence);
        if run > 0 {
            eprintln!(
                "clock-evidence run={run} iterations={ITERATIONS} elapsed_ns={nanos} storage_bytes={} allocations={}",
                size_of::<ClockEvidence>(),
                allocation.count_total
            );
        }
    }
}
