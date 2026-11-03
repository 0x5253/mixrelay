use super::ManualClock;
use crate::{Deadline, Instant, MonotonicClock, Sleeper};
use std::{thread, time::Duration};

const fn at(millis: u64) -> Instant {
    Instant::from_origin(Duration::from_millis(millis))
}

#[test]
fn the_clock_stands_still_until_advanced_and_clones_share_the_reading() {
    let clock = ManualClock::new(at(100));
    let owned_by_code = clock.clone();
    assert_eq!(owned_by_code.now(), at(100));
    assert_eq!(owned_by_code.now(), at(100));

    clock.advance(Duration::from_millis(25));
    assert_eq!(owned_by_code.now(), at(125));

    clock.advance(Duration::MAX);
    assert_eq!(owned_by_code.now(), Instant::from_origin(Duration::MAX));
}

#[test]
fn sleeping_jumps_to_a_future_deadline_and_never_moves_the_clock_back() {
    let clock = ManualClock::new(at(100));
    clock.sleep_until(Deadline::after(clock.now(), Duration::from_millis(40)));
    assert_eq!(clock.now(), at(140));

    clock.sleep_until(Deadline::at(at(120)));
    assert_eq!(clock.now(), at(140));
}

#[test]
fn a_sleep_on_a_worker_thread_moves_the_clock_the_test_reads() {
    let clock = ManualClock::default();
    let worker = {
        let clock = clock.clone();
        thread::spawn(move || clock.sleep_until(Deadline::at(at(50))))
    };
    worker.join().expect("worker thread");
    assert_eq!(clock.now(), at(50));
}
