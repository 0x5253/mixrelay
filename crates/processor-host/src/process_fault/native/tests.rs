//! Native failure payloads remain immutable and do not allocate on publication.
use super::*;

#[test]
fn raw_status_keeps_all_signed_bits_and_the_first_failure_without_rt_allocation() {
    for code in [i64::MIN, -10_867, 0, i64::MAX] {
        let failure = FirstFailure::new();
        assert_eq!(failure.read(), None);
        let first = Error::Process {
            format: "au",
            fault: ProcessFault::NativeStatus(code),
        };
        let later = Error::Process {
            format: "au",
            fault: ProcessFault::InvalidBlockSize,
        };
        let measured = allocation_counter::measure(|| {
            failure.record(&first);
            for _ in 0..32 {
                failure.record(&later);
                assert_eq!(failure.read(), Some(ProcessFault::NativeStatus(code)));
            }
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
    }
}

#[test]
fn generic_errors_do_not_invent_status_or_mask_a_later_typed_failure() {
    let failure = FirstFailure::new();
    failure.record(&Error::NotActivated);
    assert_eq!(failure.read(), None);
    failure.record(&Error::Process {
        format: "lv2",
        fault: ProcessFault::InvalidOutputEvent,
    });
    assert_eq!(failure.read(), Some(ProcessFault::InvalidOutputEvent));
}

#[test]
fn a_reserved_unfinished_publication_is_never_read_as_a_failure() {
    let failure = FirstFailure::new();
    failure.state.store(WRITING, Ordering::Release);
    failure.kind.store(8, Ordering::Relaxed);
    failure.code.store(-10_867, Ordering::Relaxed);
    assert_eq!(failure.read(), None);
    failure.state.store(READY, Ordering::Release);
    assert_eq!(failure.read(), Some(ProcessFault::NativeStatus(-10_867)));
}
