use super::*;

#[test]
fn cancellation_stops_the_only_owner_and_does_not_allow_restart() {
    let lifecycle = ProcessingLifecycle::default();
    assert_eq!(lifecycle.state(), WorkerState::NotStarted);
    let lease = lifecycle.begin().unwrap();
    assert_eq!(lifecycle.state(), WorkerState::Running);
    assert!(lifecycle.begin().is_err());

    lifecycle.request_stop();
    drop(lease);

    assert!(lifecycle.stop_requested());
    assert_eq!(lifecycle.state(), WorkerState::Stopped);
    assert!(lifecycle.begin().is_err());
}

#[test]
fn early_return_and_cancelled_unwind_are_both_failures() {
    let lifecycle = ProcessingLifecycle::default();
    drop(lifecycle.begin().unwrap());
    assert_eq!(lifecycle.state(), WorkerState::Failed);

    let lifecycle = ProcessingLifecycle::default();
    let result = std::panic::catch_unwind(|| {
        let _lease = lifecycle.begin().unwrap();
        lifecycle.request_stop();
        panic!("worker unwound during cancellation");
    });
    assert!(result.is_err());
    assert_eq!(lifecycle.state(), WorkerState::Failed);
}

#[test]
fn inspection_does_not_acknowledge_an_in_flight_callback() {
    let lifecycle = ProcessingLifecycle::default();
    let ticket = lifecycle.request_pause();

    assert!(lifecycle.pause_requested());
    assert!(!lifecycle.await_pause(ticket, Duration::ZERO, &mixrelay_time::StdClock::new()));

    assert!(lifecycle.checkpoint());
    assert!(lifecycle.await_pause(ticket, Duration::ZERO, &mixrelay_time::StdClock::new()));
    lifecycle.resume();
    assert!(!lifecycle.pause_requested());
    assert!(!lifecycle.checkpoint());
}

#[test]
fn previous_acknowledgement_cannot_authorize_a_new_transaction() {
    let lifecycle = ProcessingLifecycle::default();
    let first = lifecycle.request_pause();
    assert!(lifecycle.checkpoint());
    assert!(lifecycle.await_pause(first, Duration::ZERO, &mixrelay_time::StdClock::new()));
    lifecycle.resume();
    // The worker need not have observed resume before the next request arrives.
    let second = lifecycle.request_pause();

    assert_ne!(first, second);
    assert!(!lifecycle.await_pause(second, Duration::ZERO, &mixrelay_time::StdClock::new()));
    assert!(lifecycle.checkpoint());
    assert!(lifecycle.await_pause(second, Duration::ZERO, &mixrelay_time::StdClock::new()));
}

#[test]
fn timeout_does_not_choose_session_rollback_or_resume_policy() {
    let lifecycle = ProcessingLifecycle::default();
    let ticket = lifecycle.request_pause();

    assert!(!lifecycle.await_pause(ticket, Duration::ZERO, &mixrelay_time::StdClock::new()));
    assert!(lifecycle.pause_requested());

    lifecycle.resume();
    assert!(!lifecycle.pause_requested());
}

#[test]
fn control_wait_observes_the_processing_threads_acknowledgement() {
    let lifecycle = ProcessingLifecycle::default();
    std::thread::scope(|scope| {
        let ticket = lifecycle.request_pause();
        let worker = scope.spawn(|| assert!(lifecycle.checkpoint()));
        assert!(lifecycle.await_pause(
            ticket,
            Duration::from_secs(1),
            &mixrelay_time::StdClock::new()
        ));
        worker.join().unwrap();
    });
}
