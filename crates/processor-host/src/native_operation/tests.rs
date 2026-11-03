use super::*;

#[test]
fn every_native_stage_expires_only_while_the_same_call_remains_active() {
    let timeout = Duration::from_secs(5);
    for stage in [
        Stage::Load,
        Stage::Process,
        Stage::Parameter,
        Stage::Service,
        Stage::State,
        Stage::Editor,
        Stage::Destroy,
        Stage::EditorEvents,
    ] {
        let lane = NativeOperation::new();
        let mut watchdog = Watchdog::new(timeout);
        let start = Instant::from_origin(Duration::ZERO);
        let guard = lane.enter(stage);
        assert_eq!(watchdog.expired(lane.snapshot(), start), None);
        assert_eq!(
            watchdog.expired(
                lane.snapshot(),
                start.saturating_add(Duration::from_nanos(4_999_999_999))
            ),
            None
        );
        assert_eq!(
            watchdog.expired(lane.snapshot(), start.saturating_add(timeout)),
            Some(stage)
        );
        drop(guard);
        assert_eq!(lane.snapshot().phase(), stage.name());
        assert_eq!(lane.snapshot().active_stage(), None);
        assert_eq!(
            watchdog.expired(lane.snapshot(), start.saturating_add(timeout * 2)),
            None
        );
    }
}

#[test]
fn completed_same_stage_calls_renew_progress_even_without_an_idle_observation() {
    let lane = NativeOperation::new();
    let mut watchdog = Watchdog::new(Duration::from_secs(5));
    let start = Instant::from_origin(Duration::ZERO);
    let first = lane.enter(Stage::Process);
    let first_snapshot = lane.snapshot();
    assert_eq!(watchdog.expired(first_snapshot, start), None);
    drop(first);
    let _second = lane.enter(Stage::Process);
    assert_ne!(first_snapshot, lane.snapshot());
    assert_eq!(
        watchdog.expired(
            lane.snapshot(),
            start.saturating_add(Duration::from_secs(4))
        ),
        None
    );
    assert_eq!(
        watchdog.expired(
            lane.snapshot(),
            start.saturating_add(Duration::from_secs(8))
        ),
        None
    );
    assert_eq!(
        watchdog.expired(
            lane.snapshot(),
            start.saturating_add(Duration::from_secs(9))
        ),
        Some(Stage::Process)
    );
}

#[test]
fn progress_publication_and_observation_do_not_allocate() {
    let lane = NativeOperation::new();
    let mut watchdog = Watchdog::new(Duration::from_secs(5));
    let now = Instant::from_origin(Duration::ZERO);
    let measured = allocation_counter::measure(|| {
        for _ in 0..256 {
            let _guard = lane.enter(Stage::Process);
            assert_eq!(watchdog.expired(lane.snapshot(), now), None);
        }
    });
    assert_eq!(measured.count_total, 0);
}
