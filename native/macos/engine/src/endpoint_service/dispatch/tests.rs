use super::{Backend, Dispatch, StartError};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Create,
    Activate,
    Cancel,
    Release,
}
struct Fixture {
    fail: bool,
    events: Rc<RefCell<Vec<Event>>>,
}
impl Backend for Fixture {
    type Listener = ();
    fn listener(&self) -> Option<()> {
        self.events.borrow_mut().push(Event::Create);
        (!self.fail).then_some(())
    }
    fn activate(&self, (): &()) {
        self.events.borrow_mut().push(Event::Activate);
    }
    fn cancel_listener(&self, (): &()) {
        self.events.borrow_mut().push(Event::Cancel);
    }
    fn release_listener(&self, (): &()) {
        self.events.borrow_mut().push(Event::Release);
    }
}

#[test]
fn failed_source_listener_never_publishes_partial_ownership() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let owner = Dispatch::with_backend(Fixture {
        fail: true,
        events: Rc::clone(&events),
    });
    assert!(matches!(owner, Err(StartError::Listener)));
    assert_eq!(*events.borrow(), [Event::Create]);
}

#[test]
fn graph_source_listener_activates_once_and_cancels_before_release() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let owner = Dispatch::with_backend(Fixture {
        fail: false,
        events: Rc::clone(&events),
    })
    .unwrap();
    assert_eq!(*events.borrow(), [Event::Create, Event::Activate]);
    drop(owner);
    assert_eq!(
        *events.borrow(),
        [
            Event::Create,
            Event::Activate,
            Event::Cancel,
            Event::Release
        ]
    );
}
