use super::*;

#[test]
fn repeated_quit_holds_the_process_until_the_first_teardown_finishes() {
    let gate = ShutdownGate::default();
    assert!(!gate.requested());
    assert_eq!(gate.request_exit(), ExitRequest::Begin);
    assert!(gate.stopping());
    assert_eq!(gate.request_exit(), ExitRequest::Hold);
    gate.complete();
    assert_eq!(gate.request_exit(), ExitRequest::Allow);
    assert!(gate.stopping() && gate.finished());
}

#[test]
fn a_failed_teardown_can_be_retried_but_the_stop_request_remains() {
    let gate = ShutdownGate::default();
    assert_eq!(gate.request_exit(), ExitRequest::Begin);
    gate.attempt_failed();
    assert!(!gate.stopping());
    assert!(!gate.finished());
    assert!(gate.requested());
    assert_eq!(gate.request_exit(), ExitRequest::Begin);
}
