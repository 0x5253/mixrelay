//! Authenticated HAL inventory transport; no graph or application-capture API.
use super::settlement::Log;
use super::{MacDriver, Service};
use mixrelay_macos_transport::control_inventory::{
    create_inventory_snapshot, mr_hal_control_parse_inventory_request,
    mr_hal_control_parse_transport_replacement_request, report::Report,
};
use std::sync::{Arc, Weak};
use std::{
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
};

type Object = *mut c_void;
unsafe extern "C" {
    static _xpc_type_error: c_char;
    static _xpc_type_dictionary: c_char;
    fn xpc_get_type(object: Object) -> *const c_void;
    fn xpc_retain(object: Object) -> Object;
    fn xpc_release(object: Object);
    fn xpc_connection_get_euid(peer: Object) -> u32;
    fn xpc_connection_cancel(peer: Object);
    fn xpc_connection_send_message(peer: Object, message: Object);
}

pub(crate) struct Owned(NonNull<c_void>);
// SAFETY: XPC objects support thread-safe retain/release; control owns these refs.
unsafe impl Send for Owned {}
impl Owned {
    fn new(pointer: Object) -> Option<Self> {
        NonNull::new(pointer).map(Self)
    }
    fn raw(&self) -> Object {
        self.0.as_ptr()
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns exactly one retained or created reference.
        unsafe { xpc_release(self.raw()) };
    }
}

pub(super) type Peers = crate::peer_ledger::Peers<PeerConnection>;
pub(super) use crate::peer_ledger::Settlement;
use crate::peer_ledger::{Peer, PeerIdentity};

pub(crate) struct PeerConnection(Owned);
impl PeerConnection {
    fn raw(&self) -> Object {
        self.0.raw()
    }
}
impl Drop for PeerConnection {
    fn drop(&mut self) {
        // SAFETY: each accepted peer owns a retained live connection. Cancel
        // before its final external reference is released; queued callbacks keep
        // their native queue and weak context until they finish.
        unsafe { xpc_connection_cancel(self.raw()) };
    }
}
impl PeerIdentity for PeerConnection {
    fn identity(&self) -> usize {
        self.raw() as usize
    }
}

impl Peers {
    pub(super) fn send_inventory(&self, service: &Service) {
        let Some(message) = inventory(service) else {
            return;
        };
        for peer in self.values.iter().filter(|peer| peer.subscribed) {
            // SAFETY: this collection retains each connection and message.
            unsafe { xpc_connection_send_message(peer.object.raw(), message.raw()) };
        }
    }
}

fn audio_server(peer: Object) -> bool {
    if peer.is_null() {
        return false;
    }
    let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = [0_u8; 4096];
    let mut result = ptr::null_mut();
    // SAFETY: getpwnam_r initializes the account into caller-owned aligned
    // storage and only exposes it when status/result confirm success.
    let status = unsafe {
        libc::getpwnam_r(
            c"_coreaudiod".as_ptr(),
            account.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    status == 0 && !result.is_null() && unsafe { (*result).pw_uid == xpc_connection_get_euid(peer) }
}

fn inventory(service: &Service) -> Option<Owned> {
    let published = service.published.lock();
    let snapshot = published.as_ref()?.as_abi();
    // SAFETY: published retains every descriptor through synchronous message
    // creation; the codec duplicates them before this mutex guard is released.
    let message = Owned::new(unsafe {
        create_inventory_snapshot(
            &snapshot.entries[..snapshot.count],
            snapshot.generation,
            &service.producer_policy,
        )
    })?;
    let latencies: Vec<_> = service
        .latencies
        .lock()
        .values()
        .filter(|entry| {
            snapshot.entries[..snapshot.count]
                .iter()
                .any(|device| device.endpoint_id == entry.endpoint_id)
        })
        .copied()
        .collect();
    // SAFETY: this newly created message is exclusively owned until send.
    unsafe {
        mixrelay_macos_transport::control_inventory::write_input_latencies(
            message.raw(),
            &latencies,
        )
    }
    .then_some(message)
}

unsafe fn service_context(context: Object) -> Option<Arc<Service>> {
    // SAFETY: callers are dispatch callbacks whose live queue owns this Weak.
    unsafe { context.cast::<Weak<Service>>().as_ref() }?.upgrade()
}

/// # Safety
/// Peer is a borrowed live XPC connection from the listener block.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_driver_service_accept_peer(
    context: Object,
    peer: Object,
) -> bool {
    // SAFETY: the callback queue retains its boxed weak context.
    let Some(service) = (unsafe { service_context(context) }) else {
        return false;
    };
    if !audio_server(peer) {
        return false;
    }
    let mut peers = service.peers.lock();
    if peers.values.iter().any(|entry| entry.object.raw() == peer) {
        return true;
    }
    if peers.values.len() >= 128 || peers.values.try_reserve(1).is_err() {
        return false;
    }
    // SAFETY: listener callback retains its borrowed peer throughout this call.
    let Some(object) = Owned::new(unsafe { xpc_retain(peer) }) else {
        return false;
    };
    peers.values.push(Peer {
        object: PeerConnection(object),
        subscribed: false,
        settled: None,
        log: Log::default(),
        incompatible: false,
    });
    true
}

/// # Safety
/// Peer and event are live borrowed XPC objects for this dispatch callback.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_driver_service_peer_event(
    context: Object,
    peer: Object,
    event: Object,
) {
    // SAFETY: the callback queue retains its boxed weak context.
    let Some(service) = (unsafe { service_context(context) }) else {
        return;
    };
    if event.is_null() {
        return;
    }
    // SAFETY: event is retained by the block; the error type singleton is static.
    if unsafe { xpc_get_type(event) } == (&raw const _xpc_type_error).cast() {
        service
            .peers
            .lock()
            .values
            .retain(|entry| entry.object.raw() != peer);
        return;
    }
    let mut peers = service.peers.lock();
    let Some(entry) = peers
        .values
        .iter_mut()
        .find(|entry| entry.object.raw() == peer)
    else {
        // SAFETY: borrowed peer remains live through the callback.
        unsafe { xpc_connection_cancel(peer) };
        return;
    };
    // Only the authenticated HAL sends reports, after its Perform callbacks settled.
    // SAFETY: the dispatch block retains the event for this callback.
    if let Some(report) = unsafe { Report::decode(event) } {
        let replayed = matches!(report, Ok(Report::Replayed { .. }));
        if let Ok(Report::Unprovisioned { endpoint, clients }) = &report {
            let endpoint = endpoint.map(|byte| format!("{byte:02x}")).concat();
            if *clients == 0 {
                tracing::info!(%endpoint, "HAL clients regained their output queues");
            } else {
                tracing::warn!(%endpoint, clients,
                    "HAL clients hold IO without an output queue and are silent until the HAL retries");
            }
        }
        let published_generation = service
            .published
            .lock()
            .as_ref()
            .map(|snapshot| snapshot.as_abi().generation);
        entry.receive(published_generation, report);
        if replayed {
            peers.retire_before(peer as usize);
        }
        return;
    }
    let mut id = [0; 16];
    let mut generation = 0;
    // SAFETY: codec borrows event and writes only the exact initialized outputs.
    let request = unsafe { mr_hal_control_parse_inventory_request(event) };
    let replacement = unsafe {
        mr_hal_control_parse_transport_replacement_request(
            event,
            id.as_mut_ptr(),
            &raw mut generation,
        )
    };
    if !request && !replacement {
        return;
    }
    entry.subscribed = true;
    if replacement {
        if service
            .registry
            .callback(id)
            .is_some_and(|pin| pin.generation() == generation)
        {
            peers.replacements.insert(id, generation);
        }
    } else if let Some(message) = inventory(&service) {
        // SAFETY: both peer and the created message remain retained here.
        unsafe { xpc_connection_send_message(peer, message.raw()) };
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_driver_service_control_tick(context: Object) {
    // SAFETY: the callback queue retains its boxed weak context.
    let Some(service) = (unsafe { service_context(context) }) else {
        return;
    };
    // A transaction owns provisional inventory; never start mapping recovery
    // against it. The timer is control-only and does not own PCM publication.
    let Some(transaction) = service.transaction.try_lock() else {
        return;
    };
    let requests = std::mem::take(&mut service.peers.lock().replacements);
    drop(transaction);
    for (id, generation) in requests {
        if service
            .registry
            .callback(id)
            .is_none_or(|pin| pin.generation() != generation)
        {
            continue;
        }
        let desired = service.configuration.lock().clone();
        let mut driver = MacDriver {
            service: Arc::clone(&service),
        };
        // The next HAL heartbeat requests recovery again if still terminal.
        let _ = driver.replace_inventory_change(&desired, Some(id));
    }
    service.registry.collect();
}

#[cfg(test)]
mod tests;
