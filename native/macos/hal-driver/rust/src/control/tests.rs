use super::*;
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Counts {
    connections: Cell<usize>,
    resumed: Cell<usize>,
    released: Cell<usize>,
    requests: RefCell<Vec<Request>>,
    fail_connection: Cell<bool>,
    engine_is_live: Cell<bool>,
    liveness_reads: Cell<usize>,
    pcm_liveness: RefCell<Vec<bool>>,
}
struct Peer(Rc<Counts>);
impl Drop for Peer {
    fn drop(&mut self) {
        self.0.released.set(self.0.released.get() + 1);
    }
}
struct Fake(Rc<Counts>);
impl Backend for Fake {
    type Connection = Peer;
    fn engine_is_live(&self) -> bool {
        self.0.liveness_reads.set(self.0.liveness_reads.get() + 1);
        self.0.engine_is_live.get()
    }
    fn publish_pcm(&self, _: &Registry, engine_is_live: bool) {
        self.0.pcm_liveness.borrow_mut().push(engine_is_live);
    }
    fn connect(&self, _: *const c_void, _: u64) -> Option<Peer> {
        self.0.connections.set(self.0.connections.get() + 1);
        (!self.0.fail_connection.get()).then(|| Peer(Rc::clone(&self.0)))
    }
    fn resume(&self, _: &Peer) {
        self.0.resumed.set(self.0.resumed.get() + 1);
    }
    fn send(&self, _: &Peer, request: Request) {
        self.0.requests.borrow_mut().push(request);
    }
}

#[test]
fn driver_connection_and_inventory_refresh_do_not_require_a_desktop_lease() {
    let (owner, counts) = owner();
    owner.open();
    assert_eq!(counts.connections.get(), 1);
    owner.send(Request::Inventory);
    assert_eq!(counts.requests.borrow().len(), 2);
    heartbeat(&owner);
    assert_eq!(counts.connections.get(), 1);
    assert_eq!(counts.released.get(), 0);
    owner.disconnected(1);
    heartbeat(&owner);
    assert_eq!(counts.connections.get(), 2);
    assert_eq!(counts.released.get(), 1);
}

fn owner() -> (Control<Fake>, Rc<Counts>) {
    unsafe extern "C" fn changed(_: *mut c_void, _: u32, _: u32) {}
    unsafe extern "C" fn request(_: *mut c_void, _: u32, _: u64, _: u64) -> bool {
        false
    }
    let host = HostOps {
        context: ptr::null_mut(),
        changed,
        request_configuration: request,
    };
    // The empty registry performs no audio-device discovery or mutation, and like the
    // installed driver host it cannot inspect other processes.
    let registry = Box::new(crate::native::native_registry(
        host,
        crate::transport::ProcessInspection::SANDBOXED,
    ));
    let counts = Rc::new(Counts::default());
    (Control::new(registry, Fake(Rc::clone(&counts))), counts)
}

fn heartbeat(owner: &Control<Fake>) {
    for _ in 0..10 {
        owner.tick();
    }
}

#[test]
fn thousand_errors_and_stale_callbacks_do_not_reconnect_or_disconnect_current_peer() {
    let (owner, counts) = owner();
    owner.open();
    assert_eq!(counts.connections.get(), 1);
    assert_eq!(counts.requests.borrow().len(), 1);
    for _ in 0..1000 {
        owner.disconnected(1);
    }
    assert_eq!(owner.session.borrow().authority(), 0);
    assert_eq!(counts.connections.get(), 1);
    heartbeat(&owner);
    assert_eq!(owner.session.borrow().authority(), 2);
    for _ in 0..1000 {
        owner.disconnected(1);
    }
    assert_eq!(owner.session.borrow().authority(), 2);
    assert_eq!(counts.connections.get(), 2);
    assert_eq!(counts.released.get(), 1);
    // SAFETY: stale authority must reject before dereferencing even a null payload.
    assert!(!unsafe { owner.message(1, ptr::null_mut()) });
    owner.disconnected(2);
    // SAFETY: disconnected authority also rejects before payload inspection.
    assert!(!unsafe { owner.message(2, ptr::null_mut()) });
    heartbeat(&owner);
    assert_eq!(counts.connections.get(), 3);
    heartbeat(&owner);
    assert_eq!(counts.connections.get(), 3);
    let inventory = counts
        .requests
        .borrow()
        .iter()
        .filter(|request| **request == Request::Inventory)
        .count();
    assert_eq!(inventory, 4);
    drop(owner);
    assert_eq!(counts.released.get(), 3);
}

#[test]
fn each_connection_ends_its_settlement_replay_with_a_marker_once() {
    let (owner, counts) = owner();
    owner.open();
    heartbeat(&owner);
    let incarnation = owner.registry.incarnation;
    let marker = Request::Report(Report::Replayed {
        incarnation,
        through: 0,
    });
    let markers = |counts: &Counts| {
        counts
            .requests
            .borrow()
            .iter()
            .filter(|request| **request == marker)
            .count()
    };
    // The service attributes no settlement to a request until this marker arrives.
    assert_eq!(markers(&counts), 1);
    let authority = owner.session.borrow().authority();
    owner.disconnected(authority);
    heartbeat(&owner);
    heartbeat(&owner);
    assert_eq!(markers(&counts), 2);
}

#[test]
fn failed_connection_releases_previous_and_waits_for_full_heartbeat() {
    let (owner, counts) = owner();
    owner.open();
    owner.disconnected(1);
    counts.fail_connection.set(true);
    heartbeat(&owner);
    assert_eq!(owner.session.borrow().authority(), 0);
    assert!(owner.connection.borrow().is_none());
    assert_eq!(counts.released.get(), 1);
    assert_eq!(counts.resumed.get(), 1);
    for _ in 0..9 {
        owner.tick();
    }
    assert_eq!(counts.connections.get(), 2);
    counts.fail_connection.set(false);
    owner.tick();
    assert_eq!(owner.session.borrow().authority(), 3);
    owner.disconnected(1);
    assert_eq!(owner.session.borrow().authority(), 3);
    assert_eq!(counts.connections.get(), 3);
    assert_eq!(counts.requests.borrow().len(), 2);
}

#[test]
fn actual_registry_fence_contention_prevents_socket_publication() {
    let (owner, counts) = owner();
    let lock = owner.registry.inventory.lock().unwrap();
    owner.open();
    assert_eq!(owner.session.borrow().authority(), 0);
    assert_eq!(counts.connections.get(), 0);
    drop(lock);
    heartbeat(&owner);
    assert_eq!(owner.session.borrow().authority(), 2);
    assert_eq!(counts.connections.get(), 1);
}

#[test]
fn null_host_initialization_does_not_publish_native_work() {
    // SAFETY: null is an explicitly rejected ABI argument, with no host access.
    let mut registry = ptr::null_mut();
    assert!(!unsafe { mr_hal_control_initialize(ptr::null(), &raw mut registry) });
    assert!(registry.is_null());
}

#[test]
fn missing_client_queues_are_reported_on_each_change_and_on_recovery() {
    let (one, two) = ([1; 16], [2; 16]);
    assert_eq!(unprovisioned_changes(&vec![], &vec![(one, 2)]), [(one, 2)]);
    assert!(unprovisioned_changes(&vec![(one, 2)], &vec![(one, 2)]).is_empty());
    assert_eq!(
        unprovisioned_changes(&vec![(one, 2)], &vec![(one, 1), (two, 1)]),
        [(one, 1), (two, 1)]
    );
    assert_eq!(
        unprovisioned_changes(&vec![(one, 1), (two, 1)], &vec![(two, 1)]),
        [(one, 0)]
    );
}

#[test]
fn each_control_cycle_shares_one_liveness_observation_with_pcm_publication() {
    let (owner, counts) = owner();
    owner.tick();
    counts.engine_is_live.set(true);
    owner.tick();
    assert_eq!(counts.liveness_reads.get(), 2);
    assert_eq!(*counts.pcm_liveness.borrow(), [false, true]);
}
