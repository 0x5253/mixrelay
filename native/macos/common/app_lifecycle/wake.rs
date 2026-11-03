use std::{ffi::c_void, io, ptr::NonNull};

unsafe extern "C" {
    fn mr_application_engine_wake(request: *mut c_void) -> *mut c_void;
    fn xpc_connection_cancel(connection: *mut c_void);
    fn xpc_release(object: *mut c_void);
}

/// One native demand-start connection. Drop before releasing the controller lease,
/// so XPC cannot retry this connection after the engine has been shut down.
#[derive(Debug)]
pub struct EngineWake(NonNull<c_void>);

// SAFETY: the wrapper uniquely owns a +1 XPC reference; XPC supports cancellation
// and release on any control thread. Its static handler captures no Rust data.
unsafe impl Send for EngineWake {}

impl EngineWake {
    /// Asynchronously asks launchd to start the registered Mach service.
    /// This does not imply that the server has finished initialization.
    ///
    /// # Errors
    /// Returns an I/O error if the native client cannot be allocated.
    pub fn start() -> io::Result<Self> {
        let request = crate::control_inventory::Message::new(
            crate::control_inventory::mr_hal_control_create_inventory_request(),
        )
        .ok_or_else(|| io::Error::other("could not create engine startup request"))?;
        // SAFETY: request owns the encoded dictionary through this call; XPC
        // retains it for queued delivery. The helper transfers one active
        // connection with a static discard handler capturing no borrowed data.
        NonNull::new(unsafe { mr_application_engine_wake(request.0) })
            .map(Self)
            .ok_or_else(|| io::Error::other("could not request engine startup"))
    }
}

impl Drop for EngineWake {
    fn drop(&mut self) {
        // SAFETY: this is the unique +1 reference returned by start; cancelling
        // fences reconnects, then release relinquishes the retained connection.
        unsafe {
            xpc_connection_cancel(self.0.as_ptr());
            xpc_release(self.0.as_ptr());
        }
    }
}
