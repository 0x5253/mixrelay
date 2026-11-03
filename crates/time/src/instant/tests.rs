use super::Instant;
use std::time::Duration;

const fn at(millis: u64) -> Instant {
    Instant::from_origin(Duration::from_millis(millis))
}

#[test]
fn instants_order_by_their_distance_from_the_origin() {
    assert!(at(5) < at(7));
    assert!(Instant::from_origin(Duration::from_nanos(1)) > at(0));
}

#[test]
fn duration_since_measures_forward_and_is_zero_backward() {
    assert_eq!(at(12).duration_since(at(5)), Duration::from_millis(7));
    assert_eq!(at(5).duration_since(at(12)), Duration::ZERO);
}

#[test]
fn saturating_add_moves_forward_and_stops_at_the_latest_instant() {
    assert_eq!(at(5).saturating_add(Duration::from_millis(7)), at(12));
    let latest = Instant::from_origin(Duration::MAX);
    assert_eq!(at(5).saturating_add(Duration::MAX), latest);
    assert_eq!(latest.saturating_add(Duration::from_nanos(1)), latest);
}
