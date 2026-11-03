use super::*;

fn connected(session: &mut ControlSession) -> u64 {
    let generation = session.begin();
    assert!(session.authorize(generation, true));
    assert!(session.opened(generation, true));
    generation
}

fn heartbeat(session: &mut ControlSession) -> TickAction {
    for _ in 1..HEARTBEAT_TICKS {
        assert_eq!(session.tick(), TickAction::Maintain);
    }
    session.tick()
}

#[test]
fn new_session_waits_for_authority_then_socket_before_accepting_events() {
    let mut session = ControlSession::default();
    assert_eq!(session.phase, Phase::Disconnected);
    assert!(!session.accepts(0));
    let generation = session.begin();
    assert_eq!(generation, 1);
    assert!(!session.opened(generation, true));
    assert!(!session.accepts(generation));
    assert!(session.authorize(generation, true));
    assert!(!session.accepts(generation));
    assert!(session.opened(generation, true));
    assert!(session.accepts(generation));
}

#[test]
fn interruptions_immediately_fence_messages_without_a_reconnect_storm() {
    let mut session = ControlSession::default();
    let old = connected(&mut session);
    for _ in 0..1000 {
        session.disconnected(old);
    }
    assert!(!session.accepts(old));
    assert_eq!(session.generation, old);
    assert_eq!(heartbeat(&mut session), TickAction::Reconnect);
    let current = connected(&mut session);
    for _ in 0..1000 {
        session.disconnected(old);
    }
    assert!(session.accepts(current));
    assert!(!session.authorize(old, true));
    assert!(!session.opened(old, false));
    assert_eq!(heartbeat(&mut session), TickAction::RefreshInventory);
}

#[test]
fn rejected_authority_and_failed_socket_retry_on_the_existing_heartbeat() {
    let mut session = ControlSession::default();
    let rejected = session.begin();
    assert!(!session.authorize(rejected, false));
    assert!(!session.opened(rejected, true));
    assert_eq!(heartbeat(&mut session), TickAction::Reconnect);
    let socket_failed = session.begin();
    assert!(socket_failed > rejected);
    assert!(session.authorize(socket_failed, true));
    assert!(!session.opened(socket_failed, false));
    assert!(!session.accepts(socket_failed));
    assert_eq!(heartbeat(&mut session), TickAction::Reconnect);
    assert!(connected(&mut session) > socket_failed);
}

#[test]
fn exhausted_generation_cannot_wrap_or_reaccept_an_old_handler() {
    let mut session = ControlSession {
        generation: u64::MAX - 1,
        ..ControlSession::default()
    };
    assert_eq!(connected(&mut session), u64::MAX);
    session.disconnected(u64::MAX);
    assert_eq!(session.begin(), 0);
    assert_eq!(session.generation, u64::MAX);
    assert_eq!(session.phase, Phase::Exhausted);
    assert!(!session.opened(u64::MAX, true));
    assert!(!session.accepts(u64::MAX));
    assert!(!session.accepts(1));
    assert_eq!(heartbeat(&mut session), TickAction::Maintain);
}
