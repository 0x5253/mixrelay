use std::{
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
    sync::Weak,
};

use super::Backend;
#[cfg(test)]
mod tests;

type Object = *mut c_void;
const INTERVAL_NS: u64 = 10_000_000;
const LEEWAY_NS: u64 = 1_000_000;

unsafe extern "C" {
    static _dispatch_source_type_timer: c_char;
    static _dispatch_queue_attr_concurrent: c_char;
    fn xpc_connection_create_mach_service(name: *const c_char, queue: Object, flags: u64)
    -> Object;
    fn xpc_connection_activate(connection: Object);
    fn xpc_connection_cancel(connection: Object);
    fn xpc_release(object: Object);
    fn dispatch_queue_create(label: *const c_char, attributes: Object) -> Object;
    fn dispatch_queue_set_specific(
        queue: Object,
        key: *const c_void,
        context: Object,
        destructor: extern "C" fn(Object),
    );
    fn dispatch_set_context(object: Object, context: Object);
    fn dispatch_source_create(
        kind: *const c_char,
        handle: usize,
        mask: usize,
        queue: Object,
    ) -> Object;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_source_set_timer(timer: Object, start: u64, interval: u64, leeway: u64);
    fn dispatch_source_set_event_handler_f(source: Object, handler: extern "C" fn(Object));
    fn dispatch_activate(object: Object);
    fn dispatch_source_cancel(source: Object);
    fn dispatch_release(object: Object);
    fn mr_driver_dispatch_install_handler(listener: Object, queue: Object, context: Object);
    fn mr_driver_dispatch_install_discard_handler(listener: Object);
}

pub(in super::super) struct Native {
    queue: NonNull<c_void>,
    context: NonNull<Weak<crate::Service>>,
}

// SAFETY: the queue is a unique retained native owner, with thread-safe release.
// Its immutable context is a Weak<Service>; the queue finalizer owns its destruction
// after all listener, peer and timer callbacks have released the queue.
unsafe impl Send for Native {}

impl Native {
    pub(in super::super) fn new(service: Weak<crate::Service>) -> Option<Self> {
        // SAFETY: the SDK concurrent attribute is immortal. Separate XPC peers
        // must keep delivering ACKs while the recovery timer waits for a Perform.
        let queue = NonNull::new(unsafe {
            dispatch_queue_create(
                c"io.mixrelay.driver.control".as_ptr(),
                ptr::addr_of!(_dispatch_queue_attr_concurrent)
                    .cast_mut()
                    .cast(),
            )
        })?;
        let context = NonNull::from(Box::leak(Box::new(service)));
        // SAFETY: the queue takes ownership of this boxed Weak, releasing it only
        // when no native object or queued callback can still refer to its context.
        unsafe {
            dispatch_queue_set_specific(
                queue.as_ptr(),
                context.as_ptr().cast(),
                context.as_ptr().cast(),
                release_context,
            );
        }
        Some(Self { queue, context })
    }

    pub(in super::super) fn discard_inactive(listener: &Listener) {
        // SAFETY: failed startup owns an inactive +1 listener. An inert handler,
        // cancellation and activation make that reference releasable.
        unsafe {
            mr_driver_dispatch_install_discard_handler(listener.0.as_ptr());
            xpc_connection_cancel(listener.0.as_ptr());
            xpc_connection_activate(listener.0.as_ptr());
            xpc_release(listener.0.as_ptr());
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: listener/timer cancellation has released this owner's native
        // references. The queue remains retained by any pending native callbacks.
        unsafe { dispatch_release(self.queue.as_ptr()) };
    }
}

extern "C" fn release_context(context: Object) {
    if !context.is_null() {
        // SAFETY: the queue owns the one Box allocated by Native::new, and runs
        // its destructor after its last reference and queued callbacks disappear.
        drop(unsafe { Box::from_raw(context.cast::<Weak<crate::Service>>()) });
    }
}
pub(in super::super) struct Listener(NonNull<c_void>);
pub(in super::super) struct Timer(NonNull<c_void>);

// SAFETY: these unique +1 XPC/dispatch references permit control-thread transfer.
// Dispatch serializes activation/release; callbacks capture only the queue-owned
// weak service context, never these wrappers. The native objects themselves support thread-safe cancellation.
unsafe impl Send for Listener {}
// SAFETY: the same ownership rule applies to the timer; its handler uses only the
// queue-owned weak service context and never accesses this wrapper's storage.
unsafe impl Send for Timer {}

extern "C" fn control_tick(context: Object) {
    // SAFETY: this source retains the queue that owns context until callback return.
    unsafe { super::super::xpc::mr_driver_service_control_tick(context) };
}

impl Backend for Native {
    type Listener = Listener;
    type Timer = Timer;

    fn listener(&self) -> Option<Listener> {
        // SAFETY: the static service name and owned callback queue remain valid;
        // flag 1 creates a listener that retains its target queue.
        let listener = unsafe {
            xpc_connection_create_mach_service(
                c"io.mixrelay.audio.driver".as_ptr(),
                self.queue.as_ptr(),
                1,
            )
        };
        NonNull::new(listener).map(Listener)
    }

    fn timer(&self) -> Option<Timer> {
        // SAFETY: the SDK timer type is immortal. The created source retains the
        // serial queue that owns the weak service context.
        let timer = unsafe {
            NonNull::new(dispatch_source_create(
                &raw const _dispatch_source_type_timer,
                0,
                0,
                self.queue.as_ptr(),
            ))?
        };
        // SAFETY: creation returned a unique suspended source. Configuration is
        // completed before activation; the callback uses the queue-owned context.
        unsafe {
            dispatch_set_context(timer.as_ptr(), self.context.as_ptr().cast());
            dispatch_source_set_timer(
                timer.as_ptr(),
                dispatch_time(0, 10_000_000),
                INTERVAL_NS,
                LEEWAY_NS,
            );
            dispatch_source_set_event_handler_f(timer.as_ptr(), control_tick);
        }
        Some(Timer(timer))
    }

    fn activate(&self, listener: &Listener, timer: &Timer) {
        // SAFETY: Dispatch invokes this once after both creations succeed. The
        // XPC Blocks carry only the context retained by their target queue.
        unsafe {
            mr_driver_dispatch_install_handler(
                listener.0.as_ptr(),
                self.queue.as_ptr(),
                self.context.as_ptr().cast(),
            );
            xpc_connection_activate(listener.0.as_ptr());
            dispatch_activate(timer.0.as_ptr());
        }
    }

    fn cancel_listener(&self, listener: &Listener) {
        // SAFETY: only the activated Dispatch::drop calls this, before release.
        unsafe { xpc_connection_cancel(listener.0.as_ptr()) };
    }

    fn discard_listener(&self, listener: &Listener) {
        Self::discard_inactive(listener);
    }

    fn cancel_timer(&self, timer: &Timer) {
        // SAFETY: only the activated Dispatch::drop calls this, before release.
        unsafe { dispatch_source_cancel(timer.0.as_ptr()) };
    }

    fn release_listener(&self, listener: &Listener) {
        // SAFETY: Dispatch::drop releases this sole +1 reference only after
        // activation and cancellation. Failed startup uses discard_listener.
        unsafe { xpc_release(listener.0.as_ptr()) };
    }

    fn release_timer(&self, timer: &Timer) {
        // SAFETY: a timer can reach Dispatch::drop only after activation. Cancel
        // before releasing its unique +1 reference; no suspended source is released.
        unsafe { dispatch_release(timer.0.as_ptr()) };
    }
}
