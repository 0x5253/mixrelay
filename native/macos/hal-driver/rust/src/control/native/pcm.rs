//! A separate engine peer carries PCM grants; Driver remains the inventory owner.
use super::{Connection, Handle};
use crate::registry::Registry;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::{self, NonNull},
};

pub(super) struct Publisher {
    connection: RefCell<Option<Connection>>,
    signature: RefCell<Option<Vec<(u32, u64)>>>,
    generation: Cell<u64>,
}
impl Publisher {
    pub(super) fn new() -> Self {
        Self {
            connection: RefCell::new(None),
            signature: RefCell::new(None),
            generation: Cell::new(0),
        }
    }
    pub(super) fn publish(&self, registry: &Registry, queue: Handle, engine_is_live: bool) {
        if !engine_is_live {
            self.connection.borrow_mut().take();
            self.signature.borrow_mut().take();
            return;
        }
        if self.connection.borrow().is_none() && !self.connect(queue) {
            return;
        }
        let previous = self.signature.borrow();
        let next = crate::native::client_snapshot(registry, previous.as_deref());
        drop(previous);
        if let Some((signature, snapshot)) = next
            && let Some(connection) = self.connection.borrow().as_ref()
        {
            // SAFETY: this queue retains the peer and complete snapshot until send enqueues it.
            unsafe {
                super::xpc_connection_send_message(connection.handle.as_ptr(), snapshot.as_raw());
            };
            *self.signature.borrow_mut() = Some(signature);
        }
    }
    fn connect(&self, queue: Handle) -> bool {
        let Some(generation) = self.generation.get().checked_add(1) else {
            return false;
        };
        // SAFETY: the control owner pins Publisher and its dispatch queue before the first tick.
        let pointer = NonNull::new(unsafe {
            super::xpc_connection_create_mach_service(
                c"io.mixrelay.audio.engine".as_ptr(),
                queue,
                0,
            )
        });
        let Some(handle) = pointer else {
            return false;
        };
        let connection = Connection {
            handle,
            resumed: Cell::new(false),
        };
        // SAFETY: the copied block captures only the process-lifetime Publisher and generation.
        unsafe {
            super::mr_hal_control_install_handler(
                handle.as_ptr(),
                ptr::from_ref(self).cast(),
                generation,
                event,
            );
        };
        self.generation.set(generation);
        self.signature.borrow_mut().take();
        // SAFETY: no event is delivered on this serial queue until the current tick returns.
        unsafe { super::xpc_connection_resume(handle.as_ptr()) };
        connection.resumed.set(true);
        *self.connection.borrow_mut() = Some(connection);
        true
    }
}
unsafe extern "C" fn event(context: *const c_void, generation: u64, message: Handle) {
    // SAFETY: this event block captures the leaked control owner's stable Publisher address.
    let publisher = unsafe { &*context.cast::<Publisher>() };
    if generation != publisher.generation.get() {
        return;
    }
    if ptr::eq(
        message,
        ptr::addr_of!(super::_xpc_error_connection_interrupted).cast(),
    ) || ptr::eq(
        message,
        ptr::addr_of!(super::_xpc_error_connection_invalid).cast(),
    ) {
        publisher.connection.borrow_mut().take();
        publisher.signature.borrow_mut().take();
    }
}
