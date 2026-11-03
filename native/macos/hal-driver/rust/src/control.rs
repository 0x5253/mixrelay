//! One process-lifetime owner for the HAL registry and its serial control resources.
//! No control-state borrow crosses a registry call that can notify the HAL host.

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr,
};

use crate::{
    abi::HostOps,
    control_session::{ControlSession, TickAction},
    inventory::{
        mr_hal_core_control_connected, mr_hal_core_control_message, mr_hal_core_replacement_request,
    },
    native::mr_hal_core_create_native,
    registry::Registry,
};

mod native;

use mixrelay_macos_transport::control_inventory::report::{Report, Settled};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Inventory,
    Replacement { endpoint: [u8; 16], generation: u64 },
    Report(Report),
}

/// Endpoints with running clients that hold no queue, and how many.
type Unprovisioned = Vec<([u8; 16], u32)>;

/// Counts that differ from those `sent`; an endpoint that left the list reports 0.
fn unprovisioned_changes(sent: &Unprovisioned, current: &Unprovisioned) -> Unprovisioned {
    let recovered = sent
        .iter()
        .filter(|(endpoint, _)| current.iter().all(|(other, _)| other != endpoint))
        .map(|&(endpoint, _)| (endpoint, 0));
    let changed = current
        .iter()
        .copied()
        .filter(|entry| !sent.contains(entry));
    recovered.chain(changed).collect()
}

/// The adapter owns queue/timer handles; each connection value owns its socket.
/// Methods enqueue asynchronous events, never call back into the owner inline.
trait Backend {
    type Connection;
    fn connect(&self, owner: *const c_void, generation: u64) -> Option<Self::Connection>;
    fn resume(&self, connection: &Self::Connection);
    fn send(&self, connection: &Self::Connection, request: Request);
    fn engine_is_live(&self) -> bool;
    fn publish_pcm(&self, _registry: &Registry, _engine_is_live: bool) {}
}

struct Control<B: Backend> {
    registry: Box<Registry>,
    session: RefCell<ControlSession>,
    connection: RefCell<Option<B::Connection>>,
    backend: B,
    settled: Cell<Option<(u64, u64, crate::inventory::Settlement)>>,
    /// Authority and the newest request settlement sent on it.
    configured: Cell<(u64, u64)>,
    /// Authority and the unprovisioned client counts last sent on it.
    unprovisioned: RefCell<(u64, Unprovisioned)>,
}

impl<B: Backend> Control<B> {
    fn new(registry: Box<Registry>, backend: B) -> Self {
        Self {
            registry,
            session: RefCell::new(ControlSession::default()),
            connection: RefCell::new(None),
            backend,
            settled: Cell::new(None),
            configured: Cell::new((0, 0)),
            unprovisioned: RefCell::new((0, Vec::new())),
        }
    }

    fn registry_pointer(&self) -> *mut Registry {
        ptr::from_ref(self.registry.as_ref()).cast_mut()
    }

    fn open(&self) {
        let generation = self.session.borrow_mut().begin();
        if generation == 0 {
            return;
        }
        // Fence callbacks before cancel/release can enqueue a final stale event.
        self.connection.borrow_mut().take();
        // SAFETY: this owner retains the registry; the only control queue invokes
        // open and event delivery. The state borrow above has already ended.
        let accepted =
            unsafe { mr_hal_core_control_connected(self.registry_pointer(), generation) };
        if !self.session.borrow_mut().authorize(generation, accepted) {
            return;
        }
        let connection = self.backend.connect(ptr::from_ref(self).cast(), generation);
        if !self
            .session
            .borrow_mut()
            .opened(generation, connection.is_some())
        {
            return;
        }
        *self.connection.borrow_mut() = connection;
        if let Some(connection) = self.connection.borrow().as_ref() {
            self.backend.resume(connection);
            self.backend.send(connection, Request::Inventory);
        }
    }

    fn tick(&self) {
        // Process death releases the OS lease even if the driver service is stalled.
        // Filesystem work is confined to this serial control queue, never HAL IO.
        let engine_is_live = self.backend.engine_is_live();
        if !engine_is_live {
            self.registry.clear_latencies();
        }
        self.registry.tick();
        self.backend.publish_pcm(&self.registry, engine_is_live);
        let action = self.session.borrow_mut().tick();
        self.request_replacement();
        let authority = self.session.borrow().authority();
        // A rejected generation is reported too, so the Driver service rolls back
        // without waiting out its timeout. A later retry may still report Applied.
        if let Some((generation, settlement)) = crate::inventory::settled(&self.registry, authority)
            && self.settled.get() != Some((authority, generation, settlement))
        {
            self.send(Request::Report(Report::Inventory {
                generation,
                applied: settlement == crate::inventory::Settlement::Applied,
            }));
            self.settled.set(Some((authority, generation, settlement)));
        }
        self.send_configurations(authority);
        self.send_unprovisioned(authority);
        match action {
            TickAction::Maintain => {}
            TickAction::Reconnect => self.open(),
            TickAction::RefreshInventory => self.send(Request::Inventory),
        }
    }

    /// Reports every settled rate/format/mapping request in order, once per connection,
    /// so the Driver service can tell a rejection from a slow Perform. A new connection
    /// receives the retained settlements again, then a replay marker: the service
    /// attributes nothing to its requests until the replay has completed.
    fn send_configurations(&self, authority: u64) {
        if authority == 0 {
            return;
        }
        let (sent_authority, sent) = self.configured.get();
        let replaying = sent_authority != authority;
        let after = if replaying { 0 } else { sent };
        let incarnation = self.registry.incarnation;
        let mut newest = after;
        for settled in self.registry.settled_after(after) {
            self.send(Request::Report(Report::Settled(Settled {
                incarnation,
                endpoint: settled.endpoint,
                sequence: settled.sequence,
                rate: settled.rate,
                formats: settled.formats,
                applied: settled.applied,
            })));
            // The queue is ordered; the maximum still guards the cursor.
            newest = newest.max(settled.sequence);
        }
        if replaying {
            self.send(Request::Report(Report::Replayed {
                incarnation,
                through: newest,
            }));
        }
        self.configured.set((authority, newest));
    }

    /// Reports each change in the number of clients holding IO without a queue, so a
    /// failed re-provisioning is logged by the Driver service rather than left silent.
    fn send_unprovisioned(&self, authority: u64) {
        if authority == 0 {
            return;
        }
        let current = self.registry.unprovisioned();
        let (sent_authority, sent) = self.unprovisioned.replace((authority, Vec::new()));
        let sent = if sent_authority == authority {
            sent
        } else {
            Vec::new()
        };
        for (endpoint, clients) in unprovisioned_changes(&sent, &current) {
            self.send(Request::Report(Report::Unprovisioned { endpoint, clients }));
        }
        self.unprovisioned.replace((authority, current));
    }

    fn send(&self, request: Request) {
        if let Some(connection) = self.connection.borrow().as_ref() {
            self.backend.send(connection, request);
        }
    }

    fn request_replacement(&self) {
        let authority = self.session.borrow().authority();
        if authority == 0 {
            return;
        }
        let mut endpoint = [0; 16];
        let mut generation = 0;
        // SAFETY: the registry is retained, both stack outputs are disjoint,
        // and this query runs only on the serial control queue.
        if unsafe {
            mr_hal_core_replacement_request(
                self.registry_pointer(),
                authority,
                &raw mut endpoint,
                &raw mut generation,
            )
        } {
            self.send(Request::Replacement {
                endpoint,
                generation,
            });
        }
    }

    fn disconnected(&self, generation: u64) {
        let accepted = self.session.borrow().accepts(generation);
        self.session.borrow_mut().disconnected(generation);
        if accepted {
            self.registry.clear_latencies();
        }
    }

    /// The XPC callback retains a live event until this method returns.
    unsafe fn message(&self, generation: u64, message: *mut c_void) -> bool {
        let accepted = self.session.borrow().accepts(generation);
        // SAFETY: the native event callback retains message for this call and
        // its process-lifetime owner retains registry. No RefCell guard survives.
        accepted
            && unsafe { mr_hal_core_control_message(self.registry_pointer(), generation, message) }
    }
}

/// Publishes a registry only after both dispatch resources have been acquired.
/// The HAL factory has no shutdown callback: a successful owner lives until exit,
/// independently of COM reference counts. Failed initialization publishes nothing.
///
/// # Safety
/// `host` is null or points to readable host operations whose context/callbacks
/// remain valid for the HAL process lifetime. The SDK serializes initialization.
/// `output` is null or the exclusively writable, process-lifetime COM registry
/// slot. It must not overlap host operations or any already published owner.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_control_initialize(
    host: *const HostOps,
    output: *mut *mut Registry,
) -> bool {
    // SAFETY: Initialize supplies its stack host-operations record for this call.
    let Some(host) = (unsafe { host.as_ref() }) else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    let Some(backend) = native::Native::new() else {
        return false;
    };
    // SAFETY: create copies host operations and transfers unique registry ownership.
    let registry = unsafe { mr_hal_core_create_native(ptr::from_ref(host)) };
    if registry.is_null() {
        return false;
    }
    // SAFETY: the non-null allocation above has no other owner or published callback.
    let owner = Box::new(Control::new(unsafe { Box::from_raw(registry) }, backend));
    let owner = Box::leak(owner);
    // SAFETY: Initialize exclusively owns the static COM registry slot. Publish
    // it before queue submission, so even an immediate host callback sees it.
    unsafe { output.write(registry) };
    owner.backend.publish(owner);
    true
}

#[cfg(test)]
mod tests;
