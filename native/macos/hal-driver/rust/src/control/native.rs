use super::{Backend, Control, Request};
use mixrelay_macos_transport::control_inventory::{
    mr_hal_control_create_inventory_request, mr_hal_control_create_transport_replacement_request,
};
use std::{
    cell::Cell,
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
};

type Handle = *mut c_void;
type Callback = unsafe extern "C" fn(Handle);
mod pcm;

unsafe extern "C" {
    static _dispatch_source_type_timer: c_char;
    static _xpc_type_dictionary: c_char;
    static _xpc_error_connection_interrupted: c_char;
    static _xpc_error_connection_invalid: c_char;
    fn dispatch_queue_create(label: *const c_char, attributes: Handle) -> Handle;
    fn dispatch_source_create(
        kind: *const c_void,
        handle: usize,
        mask: usize,
        queue: Handle,
    ) -> Handle;
    fn dispatch_release(object: Handle);
    fn dispatch_resume(object: Handle);
    fn dispatch_async_f(queue: Handle, context: Handle, callback: Callback);
    fn dispatch_set_context(object: Handle, context: Handle);
    fn dispatch_source_set_event_handler_f(source: Handle, callback: Callback);
    fn dispatch_source_cancel(source: Handle);
    fn dispatch_source_set_timer(source: Handle, start: u64, interval: u64, leeway: u64);
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn xpc_connection_create_mach_service(name: *const c_char, queue: Handle, flags: u64)
    -> Handle;
    fn xpc_connection_resume(connection: Handle);
    fn xpc_connection_cancel(connection: Handle);
    fn xpc_connection_send_message(connection: Handle, message: Handle);
    fn xpc_get_type(message: Handle) -> *const c_void;
    fn xpc_release(object: Handle);
    fn mr_hal_control_install_handler(
        connection: Handle,
        owner: *const c_void,
        generation: u64,
        event: unsafe extern "C" fn(*const c_void, u64, Handle),
    );
}

struct Dispatch(NonNull<c_void>);
impl Drop for Dispatch {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns the +1 dispatch reference. Failed
        // setup never resumes/publishes it; published resources live until exit.
        unsafe { dispatch_release(self.0.as_ptr()) };
    }
}

pub(super) struct Connection {
    handle: NonNull<c_void>,
    resumed: Cell<bool>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: only the serial control queue releases its owned peer.
        // The retained event block captures a process-lifetime owner, not this handle.
        unsafe {
            xpc_connection_cancel(self.handle.as_ptr());
            if !self.resumed.get() {
                xpc_connection_resume(self.handle.as_ptr());
            }
            xpc_release(self.handle.as_ptr());
        }
    }
}

pub(super) struct Native {
    timer: Timer,
    queue: Dispatch,
    pcm: pcm::Publisher,
    pcm_pending: std::sync::atomic::AtomicBool,
}

struct Timer(Dispatch);
impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: only unpublished owners can drop. Their timer is still suspended
        // and has no handler/context. Cancel before balancing that suspension, so
        // releasing the final reference cannot publish work or trap in libdispatch.
        unsafe {
            dispatch_source_cancel(self.0.0.as_ptr());
            dispatch_resume(self.0.0.as_ptr());
        }
    }
}

impl Native {
    pub(super) fn new() -> Option<Self> {
        // SAFETY: a static NUL-terminated label and null serial attributes create
        // a privately owned queue; no callbacks are registered at this stage.
        let queue = NonNull::new(unsafe {
            dispatch_queue_create(c"io.mixrelay.hal.control".as_ptr(), ptr::null_mut())
        })
        .map(Dispatch);
        Self::with_queue(queue, |queue| {
            // SAFETY: timer type is the SDK's immutable singleton and queue is
            // retained by with_queue throughout creation. Timer handle/mask are zero.
            NonNull::new(unsafe {
                dispatch_source_create(
                    ptr::addr_of!(_dispatch_source_type_timer).cast(),
                    0,
                    0,
                    queue,
                )
            })
            .map(|pointer| Timer(Dispatch(pointer)))
        })
    }

    fn with_queue(
        queue: Option<Dispatch>,
        timer: impl FnOnce(Handle) -> Option<Timer>,
    ) -> Option<Self> {
        let queue = queue?;
        let timer = timer(queue.0.as_ptr())?;
        Some(Self {
            timer,
            queue,
            pcm: pcm::Publisher::new(),
            pcm_pending: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub(super) fn publish(&self, owner: &'static Control<Self>) {
        let context = ptr::from_ref(owner).cast_mut().cast();
        // SAFETY: the leaked owner retains this serial queue and registry until
        // process exit. Concurrent SDK control calls only enqueue the PCM event.
        let signal = unsafe { crate::control_signal::ControlSignal::new(context, request_pcm) };
        let _ = owner.registry.control_signal.set(signal);
        // SAFETY: Box::leak pins the complete owner before any enqueue. Timer and
        // XPC callbacks share this serial queue; initialization occurs there too.
        unsafe {
            dispatch_set_context(self.timer.0.0.as_ptr(), context);
            dispatch_source_set_event_handler_f(self.timer.0.0.as_ptr(), tick);
            dispatch_source_set_timer(
                self.timer.0.0.as_ptr(),
                dispatch_time(0, 100_000_000),
                100_000_000,
                10_000_000,
            );
            dispatch_async_f(self.queue.0.as_ptr(), context, start);
        }
    }
}

impl Backend for Native {
    type Connection = Connection;

    fn engine_is_live(&self) -> bool {
        use mixrelay_macos_transport::app_lifecycle::{ENGINE_LEASE, is_owned};
        is_owned(ENGINE_LEASE).unwrap_or(false)
    }
    fn publish_pcm(&self, registry: &crate::registry::Registry, engine_is_live: bool) {
        self.pcm
            .publish(registry, self.queue.0.as_ptr(), engine_is_live);
    }

    fn connect(&self, owner: *const c_void, generation: u64) -> Option<Connection> {
        // SAFETY: the owner's only caller is Control::open on this queue. Its
        // address is process-lifetime stable before the connection can be resumed.
        let connection = NonNull::new(unsafe {
            xpc_connection_create_mach_service(
                c"io.mixrelay.audio.driver".as_ptr(),
                self.queue.0.as_ptr(),
                0,
            )
        })?;
        // SAFETY: the helper copies the block capturing only owner/generation;
        // no Rust reference or borrowed connection is captured by the block.
        unsafe { mr_hal_control_install_handler(connection.as_ptr(), owner, generation, event) };
        Some(Connection {
            handle: connection,
            resumed: Cell::new(false),
        })
    }

    fn resume(&self, connection: &Connection) {
        // SAFETY: Control::open resumes its new connection exactly once, after
        // committing authority and installing the event handler.
        if !connection.resumed.replace(true) {
            unsafe { xpc_connection_resume(connection.handle.as_ptr()) };
        }
    }

    fn send(&self, connection: &Connection, request: Request) {
        let message = match request {
            Request::Inventory => mr_hal_control_create_inventory_request(),
            Request::Report(report) => report.encode(),
            Request::Replacement {
                endpoint,
                generation,
            } => {
                // SAFETY: endpoint is a live 16-byte local, synchronously copied.
                unsafe {
                    mr_hal_control_create_transport_replacement_request(
                        endpoint.as_ptr(),
                        generation,
                    )
                }
            }
        };
        if !message.is_null() {
            // SAFETY: the creator transfers one reference. send retains message
            // for asynchronous delivery before this scope releases its own reference.
            unsafe {
                xpc_connection_send_message(connection.handle.as_ptr(), message);
                xpc_release(message);
            }
        }
    }
}

unsafe extern "C" fn start(context: Handle) {
    // SAFETY: publish enqueues the leaked owner exclusively on its serial queue.
    let owner = unsafe { &*context.cast::<Control<Native>>() };
    owner.open();
    // SAFETY: timer is owned and suspended until this one-shot start callback.
    unsafe { dispatch_resume(owner.backend.timer.0.0.as_ptr()) };
}

unsafe extern "C" fn tick(context: Handle) {
    // SAFETY: timer's context is the leaked owner; delivery is serial with XPC.
    unsafe { &*context.cast::<Control<Native>>() }.tick();
}

unsafe extern "C" fn request_pcm(context: *const c_void) {
    // SAFETY: only the immutable queue handle is read concurrently; Publisher's
    // RefCells are accessed exclusively by the subsequently enqueued callback.
    let owner = unsafe { &*context.cast::<Control<Native>>() };
    // Coalesce concurrent SDK control events; registry mutexes publish the actual
    // membership, so the flag only owns one pending dispatch callback.
    if owner
        .backend
        .pcm_pending
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    unsafe {
        dispatch_async_f(
            owner.backend.queue.0.as_ptr(),
            context.cast_mut(),
            pcm_changed,
        );
    };
}

unsafe extern "C" fn pcm_changed(context: Handle) {
    // SAFETY: the process-lifetime owner receives this event on its serial queue.
    let owner = unsafe { &*context.cast::<Control<Native>>() };
    owner
        .backend
        .pcm_pending
        .store(false, std::sync::atomic::Ordering::Release);
    owner
        .backend
        .publish_pcm(&owner.registry, owner.backend.engine_is_live());
}

unsafe extern "C" fn event(context: *const c_void, generation: u64, message: Handle) {
    // SAFETY: XPC retains its event for this callback; the block captures the
    // leaked owner and delivers on its sole queue, including stale final events.
    let owner = unsafe { &*context.cast::<Control<Native>>() };
    if message.is_null() {
        return;
    }
    // SAFETY: XPC owns a non-null event. Type/error symbols are immutable objects.
    if unsafe { xpc_get_type(message) } == ptr::addr_of!(_xpc_type_dictionary).cast() {
        // SAFETY: event remains borrowed for this synchronous registry call.
        unsafe { owner.message(generation, message) };
    } else if ptr::eq(
        message,
        ptr::addr_of!(_xpc_error_connection_interrupted).cast(),
    ) || ptr::eq(message, ptr::addr_of!(_xpc_error_connection_invalid).cast())
    {
        owner.disconnected(generation);
    }
}

#[cfg(test)]
mod tests;
