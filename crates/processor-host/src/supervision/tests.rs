use super::*;
use crate::native_operation::NativeOperation;

fn activity(
    creator: &NativeOperation,
    dsp: &NativeOperation,
    parent_request_pending: bool,
) -> NativeActivity {
    NativeActivity {
        creator: creator.snapshot(),
        dsp: dsp.snapshot(),
        parent_request_pending,
    }
}

#[test]
fn editor_interaction_waits_for_a_parent_request_but_never_masks_a_stuck_dsp_call() {
    let creator = NativeOperation::new();
    let dsp = NativeOperation::new();
    let timeout = Duration::from_secs(5);
    let now = Instant::from_origin(Duration::ZERO);
    let mut supervisor = Supervisor::new(timeout, Duration::from_secs(2), now);
    let _interaction = creator.enter(Stage::EditorEvents);
    assert_eq!(
        supervisor.native_timeout(activity(&creator, &dsp, false), now),
        None
    );
    assert_eq!(
        supervisor.native_timeout(
            activity(&creator, &dsp, false),
            now.saturating_add(timeout * 10)
        ),
        None
    );
    assert_eq!(
        supervisor.native_timeout(
            activity(&creator, &dsp, true),
            now.saturating_add(timeout * 10)
        ),
        None
    );
    assert_eq!(
        supervisor.native_timeout(
            activity(&creator, &dsp, true),
            now.saturating_add(timeout * 11)
        ),
        Some(Stage::EditorEvents)
    );
    let _processing = dsp.enter(Stage::Process);
    assert_eq!(
        supervisor.native_timeout(
            activity(&creator, &dsp, false),
            now.saturating_add(timeout * 12)
        ),
        None
    );
    assert_eq!(
        supervisor.native_timeout(
            activity(&creator, &dsp, false),
            now.saturating_add(timeout * 13)
        ),
        Some(Stage::Process)
    );
}

#[test]
fn child_exit_and_wait_failures_keep_their_meaning_during_shutdown() {
    let creator = NativeOperation::new();
    let dsp = NativeOperation::new();
    let now = Instant::from_origin(Duration::ZERO);
    for (child, stop, expected) in [
        (
            ChildState::Exited,
            StopStatus::Running,
            Action::Finished { unexpected: true },
        ),
        (
            ChildState::Exited,
            StopStatus::Requested,
            Action::Finished { unexpected: false },
        ),
        (
            ChildState::WaitFailed,
            StopStatus::Running,
            Action::Kill {
                failure: Some(Failure::ChildWait),
            },
        ),
        (
            ChildState::WaitFailed,
            StopStatus::Requested,
            Action::Kill {
                failure: Some(Failure::ChildWait),
            },
        ),
    ] {
        let mut supervisor = Supervisor::new(Duration::from_secs(5), Duration::ZERO, now);
        assert_eq!(
            supervisor.observe(child, stop, activity(&creator, &dsp, false), now),
            expected
        );
    }
}

#[test]
fn shutdown_grace_begins_at_the_first_stop_and_repeated_observations_do_not_extend_it() {
    let creator = NativeOperation::new();
    let dsp = NativeOperation::new();
    let idle = activity(&creator, &dsp, false);
    let now = Instant::from_origin(Duration::ZERO);
    let grace = Duration::from_secs(2);
    let mut supervisor = Supervisor::new(Duration::from_secs(5), grace, now);
    let stop = now.saturating_add(Duration::from_secs(20));
    assert_eq!(
        supervisor.observe(ChildState::Running, StopStatus::Running, idle, stop),
        Action::Continue
    );
    assert_eq!(
        supervisor.observe(ChildState::Running, StopStatus::Requested, idle, stop),
        Action::Continue
    );
    let before = stop.saturating_add(grace.checked_sub(Duration::from_nanos(1)).unwrap());
    assert_eq!(
        supervisor.observe(ChildState::Running, StopStatus::Requested, idle, before),
        Action::Continue
    );
    assert_eq!(
        supervisor.observe(
            ChildState::Running,
            StopStatus::Requested,
            idle,
            stop.saturating_add(grace)
        ),
        Action::Kill { failure: None }
    );
}

#[test]
fn native_timeout_is_reported_when_its_deadline_coincides_with_shutdown_grace() {
    let creator = NativeOperation::new();
    let dsp = NativeOperation::new();
    let _processing = dsp.enter(Stage::Process);
    let running = activity(&creator, &dsp, false);
    let now = Instant::from_origin(Duration::ZERO);
    let timeout = Duration::from_secs(5);
    let mut supervisor = Supervisor::new(timeout, timeout, now);
    assert_eq!(
        supervisor.observe(ChildState::Running, StopStatus::Requested, running, now),
        Action::Continue
    );
    assert_eq!(
        supervisor.observe(
            ChildState::Running,
            StopStatus::Requested,
            running,
            now.saturating_add(timeout)
        ),
        Action::Kill {
            failure: Some(Failure::NativeTimeout(Stage::Process))
        }
    );
}

#[test]
fn delayed_timing_observations_request_one_sample_without_catch_up_bursts() {
    let now = Instant::from_origin(Duration::ZERO);
    let mut supervisor = Supervisor::new(Duration::from_secs(5), Duration::from_secs(2), now);
    assert!(supervisor.sample_due(now));
    assert!(!supervisor.sample_due(now.saturating_add(Duration::from_nanos(999_999_999))));
    assert!(supervisor.sample_due(now.saturating_add(Duration::from_secs(1))));
    let delayed = now.saturating_add(Duration::from_secs(20));
    assert!(supervisor.sample_due(delayed));
    assert!(!supervisor.sample_due(delayed));
}
