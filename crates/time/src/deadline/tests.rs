use super::Deadline;
use crate::Instant;
use std::time::Duration;

const fn at(millis: u64) -> Instant {
    Instant::from_origin(Duration::from_millis(millis))
}

#[test]
fn a_deadline_after_a_timeout_counts_down_and_passes_at_its_instant() {
    let deadline = Deadline::after(at(10), Duration::from_millis(5));
    assert_eq!(deadline, Deadline::at(at(15)));
    assert_eq!(deadline.instant(), at(15));

    assert_eq!(deadline.remaining(at(12)), Duration::from_millis(3));
    assert!(!deadline.has_passed(at(14)));

    assert_eq!(deadline.remaining(at(15)), Duration::ZERO);
    assert!(deadline.has_passed(at(15)));
    assert_eq!(deadline.remaining(at(40)), Duration::ZERO);
    assert!(deadline.has_passed(at(40)));
}

#[test]
fn an_unrepresentable_timeout_saturates_instead_of_wrapping_into_the_past() {
    let deadline = Deadline::after(at(10), Duration::MAX);
    assert!(!deadline.has_passed(at(10)));
    assert!(!deadline.has_passed(Instant::from_origin(Duration::from_secs(u64::MAX))));
    assert_eq!(deadline.instant(), Instant::from_origin(Duration::MAX));
}

#[test]
fn deadlines_order_earliest_first() {
    let mut deadlines = [
        Deadline::at(at(30)),
        Deadline::at(at(10)),
        Deadline::at(at(20)),
    ];
    deadlines.sort();
    assert_eq!(deadlines.map(Deadline::instant), [at(10), at(20), at(30)]);
}
