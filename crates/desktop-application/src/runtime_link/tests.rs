use super::*;
use mixrelay_time::{ManualClock, MonotonicClock};
#[test]
fn active_routes_suspend_idle_retirement_and_last_completion_starts_the_window() {
    let clock = ManualClock::default();
    let mut routes = RouteTable::new(clock.now());
    let reuse = Duration::from_secs(10);
    routes.register(1, "first", clock.now(), reuse).unwrap();
    clock.advance(Duration::from_mins(1));
    routes.register(2, "second", clock.now(), reuse).unwrap();
    routes.remove(1, clock.now());
    clock.advance(Duration::from_mins(1));
    routes.remove(2, clock.now());
    clock.advance(reuse.checked_sub(Duration::from_nanos(1)).unwrap());
    routes.register(3, "inside", clock.now(), reuse).unwrap();
    routes.remove(3, clock.now());
    clock.advance(reuse);
    assert_eq!(
        routes.register(4, "too late", clock.now(), reuse),
        Err(AdmissionFailure::IdleExpired)
    );
}
#[test]
fn close_drains_once_and_never_readmits_a_route() {
    let clock = ManualClock::default();
    let mut routes = RouteTable::new(clock.now());
    routes
        .register(1, "waiting", clock.now(), Duration::from_secs(10))
        .unwrap();
    assert_eq!(routes.close().get(&1), Some(&"waiting"));
    assert!(routes.close().is_empty());
    assert_eq!(
        routes.register(2, "late", clock.now(), Duration::from_secs(10)),
        Err(AdmissionFailure::Closed)
    );
}
#[test]
fn heartbeats_do_not_replace_terminal_errors_or_success() {
    use serde_json::json;
    assert_eq!(
        command_frame(json!({"status":102})),
        CommandFrame::Heartbeat
    );
    assert_eq!(
        command_frame(json!({"status":200,"body":{"error":{"code":"busy"}}})),
        CommandFrame::Complete(Err(json!({"code":"busy"})))
    );
    assert_eq!(
        command_frame(json!({"status":200,"body":{"ok":null}})),
        CommandFrame::Complete(Ok(Value::Null))
    );
    assert_eq!(
        command_frame(json!({"status":200,"body":{}})),
        CommandFrame::InvalidResponse
    );
}
