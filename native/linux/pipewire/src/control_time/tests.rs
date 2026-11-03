use super::*;
use mixrelay_time::ManualClock;

#[test]
fn readiness_snapshots_return_values_and_failure_without_real_time_or_native_objects() {
    let clock = ManualClock::default();
    let mut calls = 0;
    let actual = wait_for(&clock, || {
        calls += 1;
        if calls < 3 {
            Observation::Pending
        } else {
            Observation::Ready(17)
        }
    })
    .unwrap();
    assert_eq!(actual, 17);
    assert_eq!(
        clock
            .now()
            .duration_since(mixrelay_time::Instant::from_origin(Duration::ZERO)),
        Duration::from_millis(4)
    );
    assert_eq!(
        wait_for::<()>(&clock, || Observation::Failed),
        Err(WaitError::Failed)
    );
}

#[test]
fn pending_snapshots_stop_at_the_deadline_and_ready_evidence_wins_at_the_boundary() {
    let clock = ManualClock::default();
    assert_eq!(
        wait_for::<()>(&clock, || Observation::Pending),
        Err(WaitError::TimedOut)
    );
    let start = clock.now();
    assert_eq!(
        start.duration_since(mixrelay_time::Instant::from_origin(Duration::ZERO)),
        PREPARATION_TIMEOUT
    );
    assert_eq!(
        wait_for(&clock, || {
            if clock.now().duration_since(start) == PREPARATION_TIMEOUT {
                Observation::Ready(7)
            } else {
                Observation::Pending
            }
        }),
        Ok(7)
    );
}
