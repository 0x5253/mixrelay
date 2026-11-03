use std::{cell::RefCell, rc::Rc};

use super::{Backend, Dispatch, StartError};

#[derive(Debug, PartialEq, Eq)]
enum Event {
    CreateListener,
    CreateTimer,
    Activate,
    DiscardInactiveListener,
    CancelListener,
    CancelTimer,
    ReleaseListener,
    ReleaseTimer,
}

struct Fixture {
    fail: Option<StartError>,
    events: Rc<RefCell<Vec<Event>>>,
}

impl Backend for Fixture {
    type Listener = ();
    type Timer = ();

    fn listener(&self) -> Option<()> {
        self.events.borrow_mut().push(Event::CreateListener);
        (self.fail != Some(StartError::Listener)).then_some(())
    }

    fn timer(&self) -> Option<()> {
        self.events.borrow_mut().push(Event::CreateTimer);
        (self.fail != Some(StartError::Timer)).then_some(())
    }

    fn activate(&self, (): &(), (): &()) {
        self.events.borrow_mut().push(Event::Activate);
    }

    fn cancel_listener(&self, (): &()) {
        self.events.borrow_mut().push(Event::CancelListener);
    }

    fn discard_listener(&self, (): &()) {
        self.events
            .borrow_mut()
            .push(Event::DiscardInactiveListener);
    }

    fn cancel_timer(&self, (): &()) {
        self.events.borrow_mut().push(Event::CancelTimer);
    }

    fn release_listener(&self, (): &()) {
        self.events.borrow_mut().push(Event::ReleaseListener);
    }

    fn release_timer(&self, (): &()) {
        self.events.borrow_mut().push(Event::ReleaseTimer);
    }
}

#[test]
fn listener_failure_does_not_create_an_unused_timer() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let owner = Dispatch::with_backend(Fixture {
        fail: Some(StartError::Listener),
        events: events.clone(),
    });
    assert!(matches!(owner, Err(StartError::Listener)));
    assert_eq!(*events.borrow(), [Event::CreateListener]);
}

#[test]
fn timer_failure_discards_inactive_listener_before_release() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let owner = Dispatch::with_backend(Fixture {
        fail: Some(StartError::Timer),
        events: events.clone(),
    });
    assert!(matches!(owner, Err(StartError::Timer)));
    assert_eq!(
        *events.borrow(),
        [
            Event::CreateListener,
            Event::CreateTimer,
            Event::DiscardInactiveListener
        ]
    );
}

#[test]
fn complete_owner_activates_once_and_cancels_before_release() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let owner = Dispatch::with_backend(Fixture {
        fail: None,
        events: events.clone(),
    })
    .unwrap();
    assert_eq!(
        *events.borrow(),
        [Event::CreateListener, Event::CreateTimer, Event::Activate]
    );
    drop(owner);
    assert_eq!(
        *events.borrow(),
        [
            Event::CreateListener,
            Event::CreateTimer,
            Event::Activate,
            Event::CancelListener,
            Event::CancelTimer,
            Event::ReleaseListener,
            Event::ReleaseTimer
        ]
    );
}

#[test]
fn repeated_failed_starts_and_recovery_leave_no_partial_owner() {
    for _ in 0..1_000 {
        timer_failure_discards_inactive_listener_before_release();
        complete_owner_activates_once_and_cancels_before_release();
    }
}
