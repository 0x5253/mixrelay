use super::Backend;
use std::{
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
};
#[cfg(test)]
mod tests;
type Object = *mut c_void;

unsafe extern "C" {
    fn xpc_connection_create_mach_service(name: *const c_char, queue: Object, flags: u64)
    -> Object;
    fn xpc_connection_activate(connection: Object);
    fn xpc_connection_cancel(connection: Object);
    fn xpc_release(object: Object);
    fn mr_engine_dispatch_install_handler(listener: Object);
}

pub(in super::super) struct Native;
pub(in super::super) struct Listener(NonNull<c_void>);

// SAFETY: Dispatch uniquely owns the +1 XPC reference. Callbacks retain the
// process-lifetime service rather than borrowing Listener; XPC cancellation is thread safe.
unsafe impl Send for Listener {}

impl Backend for Native {
    type Listener = Listener;

    fn listener(&self) -> Option<Listener> {
        // SAFETY: static terminated service name, default dispatch queue and
        // listener flag. A non-null result owns one inactive +1 XPC reference.
        NonNull::new(unsafe {
            xpc_connection_create_mach_service(
                c"io.mixrelay.audio.engine".as_ptr(),
                ptr::null_mut(),
                1,
            )
        })
        .map(Listener)
    }

    fn activate(&self, listener: &Listener) {
        // SAFETY: sole inactive owner, activated exactly once after its static handler.
        unsafe {
            mr_engine_dispatch_install_handler(listener.0.as_ptr());
            xpc_connection_activate(listener.0.as_ptr());
        }
    }

    fn cancel_listener(&self, listener: &Listener) {
        // SAFETY: Dispatch calls cancellation before releasing this sole activated owner.
        unsafe { xpc_connection_cancel(listener.0.as_ptr()) };
    }

    fn release_listener(&self, listener: &Listener) {
        // SAFETY: activated/cancelled owner retains this unique +1 reference.
        unsafe { xpc_release(listener.0.as_ptr()) };
    }
}
