use super::StdClock;
use crate::{Deadline, MonotonicClock, Sleeper};
use std::time::Duration;

#[test]
fn readings_never_go_back_and_sleeping_reaches_the_deadline() {
    let clock = StdClock::new();
    let start = clock.now();
    let deadline = Deadline::after(start, Duration::from_millis(5));
    clock.sleep_until(deadline);
    assert!(deadline.has_passed(clock.now()));
    clock.sleep_until(Deadline::at(start));
}
