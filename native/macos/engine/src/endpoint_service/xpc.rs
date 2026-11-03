//! XPC policy is control-plane Rust. C only provides Objective-C block adapters.
use super::SERVICE;
use mixrelay_macos_transport::source_control::{
    SourceRequest, mr_engine_control_create_source_attachment_reply,
    mr_engine_control_parse_source_request,
};
use std::{
    ffi::{c_char, c_void},
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    ptr::NonNull,
};
type Object = *mut c_void;
unsafe extern "C" {
    static _xpc_type_error: c_char;
    fn xpc_get_type(object: Object) -> *const c_void;
    fn xpc_retain(object: Object) -> Object;
    fn xpc_release(object: Object);
    fn xpc_connection_get_euid(peer: Object) -> u32;
    fn xpc_connection_cancel(peer: Object);
    fn xpc_connection_send_message(peer: Object, message: Object);
    fn xpc_dictionary_create_reply(request: Object) -> Object;
}
struct Owned(NonNull<c_void>);
// SAFETY: XPC retains/releases are thread safe; use is restricted to control.
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
        unsafe { xpc_release(self.raw()) };
    }
}
struct Peer {
    object: Owned,
    role: Role,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Application,
    AudioServer,
}
#[derive(Default)]
pub(super) struct Peers {
    values: Vec<Peer>,
}

fn authorized(peer: Object) -> Option<Role> {
    if peer.is_null() {
        return None;
    }
    // SAFETY: the listener supplies a retained live peer for this control call.
    let uid = unsafe { xpc_connection_get_euid(peer) };
    if std::fs::metadata("/dev/console").is_ok_and(|metadata| metadata.uid() == uid) {
        return Some(Role::Application);
    }
    let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = [0_u8; 4096];
    let mut result = std::ptr::null_mut();
    // SAFETY: the system account lookup fills only the supplied storage; its pointer
    // is read only when both status and result confirm a complete account record.
    let status = unsafe {
        libc::getpwnam_r(
            c"_coreaudiod".as_ptr(),
            account.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    (status == 0 && !result.is_null() && unsafe { (*result).pw_uid } == uid)
        .then_some(Role::AudioServer)
}

/// # Safety
/// Peer is a borrowed live XPC connection from the listener adapter.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_service_accept_peer(peer: Object) -> bool {
    let Some(service) = SERVICE.get() else {
        return false;
    };
    let Some(role) = authorized(peer) else {
        return false;
    };
    let mut peers = service.peers.lock();
    if peers.values.iter().any(|entry| entry.object.raw() == peer) {
        return true;
    }
    if peers.values.len() == 128 {
        return false;
    }
    let Some(object) = Owned::new(unsafe { xpc_retain(peer) }) else {
        return false;
    };
    if role == Role::AudioServer
        && peers
            .values
            .iter()
            .any(|peer| peer.role == Role::AudioServer)
    {
        return false;
    }
    peers.values.push(Peer { object, role });
    true
}

fn source_reply(event: Object) -> Option<Owned> {
    let service = SERVICE.get()?;
    let mut request = SourceRequest {
        source_id: [0; 128],
        capacity_frames: 0,
        channels: 0,
        transport_version: 0,
    };
    if !unsafe { mr_engine_control_parse_source_request(event, &raw mut request) } {
        return None;
    }
    let Ok(attachment) = service.sources.attach(&request) else {
        // SAFETY: this borrowed dictionary request is retained for the callback;
        // an empty owned reply lets the source client reject the failed attachment.
        return Owned::new(unsafe { xpc_dictionary_create_reply(event) });
    };
    Owned::new(unsafe {
        mr_engine_control_create_source_attachment_reply(
            event,
            attachment.descriptor.as_raw_fd(),
            attachment.mapped_size,
            attachment.producer_lease,
            attachment.source_epoch,
        )
    })
}

/// # Safety
/// Both values are live borrowed XPC objects for this dispatch callback.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_service_peer_event(peer: Object, event: Object) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    if event.is_null() {
        return;
    }
    if unsafe { xpc_get_type(event) } == (&raw const _xpc_type_error).cast() {
        let mut peers = service.peers.lock();
        let was_audio_server = peers
            .values
            .iter()
            .any(|entry| entry.object.raw() == peer && entry.role == Role::AudioServer);
        peers.values.retain(|entry| entry.object.raw() != peer);
        drop(peers);
        if was_audio_server {
            service.virtual_pcm.clear();
        }
        return;
    }
    let role = service
        .peers
        .lock()
        .values
        .iter()
        .find(|entry| entry.object.raw() == peer)
        .map(|entry| entry.role);
    let Some(role) = role else {
        // SAFETY: peer remains retained through this listener callback.
        unsafe { xpc_connection_cancel(peer) };
        return;
    };
    if role == Role::AudioServer {
        // SAFETY: the authenticated peer retains the complete request during decoding.
        let result = unsafe { virtual_snapshot(service, event) };
        if let Err(error) = result {
            tracing::error!(boundary = "hal_shared_pcm", stage = "grant_snapshot", %error, "virtual input snapshot rejected");
            // SAFETY: cancellation asks HAL to reconnect and submit its complete inventory.
            unsafe { xpc_connection_cancel(peer) };
        }
        return;
    }
    if let Some(response) = source_reply(event) {
        // SAFETY: response retains the owned reply until this synchronous send.
        unsafe { xpc_connection_send_message(peer, response.raw()) };
    }
}

/// # Safety
/// Event is a live envelope from the authenticated audio-server peer.
unsafe fn virtual_snapshot(
    service: &super::Service,
    event: Object,
) -> Result<(), super::virtual_pcm::Error> {
    use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;
    let received = mixrelay_realtime::host_time();
    if service.inventory.stopped() {
        return Err(super::virtual_pcm::Error::Stale);
    }
    // SAFETY: the caller retains this authenticated envelope; decode checks its
    // dictionary/version before the remaining accessors inspect its fields.
    let (queues, geometry, observed) = unsafe {
        (
            ClientPcmSnapshot::decode(event)?,
            ClientPcmSnapshot::endpoints(event)?,
            ClientPcmSnapshot::observed_at(event),
        )
    };
    if queues
        .iter()
        .any(|queue| queue.capacity() != mixrelay_macos_transport::client_pcm::QUEUE_FRAMES)
    {
        return Err(super::virtual_pcm::Error::Geometry);
    }
    let pending = service
        .virtual_pcm
        .replace_ready(queues, observed, received)?;
    service.inventory.submit(
        std::sync::Arc::clone(&service.virtual_pcm),
        super::inventory::Job {
            geometry,
            pending,
            observed,
            received,
        },
    )?;
    Ok(())
}
