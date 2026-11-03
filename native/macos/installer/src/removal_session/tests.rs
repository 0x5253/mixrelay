use super::*;

#[test]
fn only_the_committed_peer_can_acknowledge_and_retirement_happens_once() {
    for status in [0, 2] {
        let mut session = RemovalSession::default();
        assert_eq!(session.event(Event::Admission, 7), Action::AllowRemoval);
        assert_eq!(
            session.event(Event::Transaction(status), 7),
            Action::AwaitAcknowledgement
        );
        assert_eq!(session.event(Event::Admission, 8), Action::Ignore);
        assert_eq!(session.event(Event::Acknowledged, 8), Action::Ignore);
        assert_eq!(session.event(Event::Disconnected, 8), Action::Ignore);
        assert_eq!(session.event(Event::IdleDeadline, 0), Action::Ignore);
        assert_eq!(session.event(Event::Acknowledged, 7), Action::Finish);
        for event in [
            Event::Acknowledged,
            Event::Disconnected,
            Event::Abandoned,
            Event::Terminated,
        ] {
            assert_eq!(session.event(event, 7), Action::Ignore);
        }
    }
}

#[test]
fn committed_disconnect_or_abandonment_retires_without_an_acknowledgement() {
    for event in [Event::Disconnected, Event::Abandoned] {
        let mut session = RemovalSession::default();
        session.event(Event::Transaction(0), 7);
        assert_eq!(session.event(event, 7), Action::Finish);
        assert_eq!(session.event(event, 7), Action::Ignore);
    }
}

#[test]
fn rejected_request_can_retry_but_idle_or_terminated_helpers_only_clean_the_private_copy() {
    let mut session = RemovalSession::default();
    assert_eq!(session.event(Event::Transaction(1), 7), Action::AwaitIdle);
    assert_eq!(session.event(Event::Disconnected, 7), Action::Ignore);
    assert_eq!(session.event(Event::Admission, 8), Action::AllowRemoval);
    assert_eq!(session.event(Event::IdleDeadline, 0), Action::Idle);
    assert_eq!(session.event(Event::Admission, 8), Action::Ignore);
    let mut committed = RemovalSession::default();
    committed.event(Event::Transaction(2), 7);
    assert_eq!(committed.event(Event::Terminated, 0), Action::Idle);
    assert_eq!(committed.event(Event::Abandoned, 0), Action::Ignore);
}
