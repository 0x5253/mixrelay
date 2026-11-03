use std::{
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use super::{Backend, Listener, Native, Object};
use crate::endpoint_service::dispatch::Dispatch;

unsafe extern "C" {
    fn xpc_connection_create(name: *const std::ffi::c_char, queue: Object) -> Object;
    fn xpc_connection_set_context(connection: Object, context: Object);
    fn xpc_connection_set_finalizer_f(connection: Object, finalizer: extern "C" fn(Object));
}

static FINALIZED: AtomicUsize = AtomicUsize::new(0);

extern "C" fn finalized(_context: Object) {
    FINALIZED.fetch_add(1, Ordering::Release);
}

struct Fixture;

impl Backend for Fixture {
    type Listener = Listener;

    fn listener(&self) -> Option<Listener> {
        // SAFETY: null name creates an anonymous listener with no Mach service
        // registration. The result owns one inactive +1 reference, as production.
        let pointer = unsafe { xpc_connection_create(ptr::null(), ptr::null_mut()) };
        let pointer = std::ptr::NonNull::new(pointer)?;
        // SAFETY: XPC only passes the immortal static context to finalized, which
        // never dereferences it. A non-null context enables the SDK finalizer.
        unsafe {
            xpc_connection_set_context(
                pointer.as_ptr(),
                ptr::from_ref(&FINALIZED).cast_mut().cast(),
            );
            xpc_connection_set_finalizer_f(pointer.as_ptr(), finalized);
        }
        Some(Listener(pointer))
    }

    fn activate(&self, listener: &Listener) {
        Native.activate(listener);
    }

    fn cancel_listener(&self, listener: &Listener) {
        Native.cancel_listener(listener);
    }

    fn release_listener(&self, listener: &Listener) {
        Native.release_listener(listener);
    }
}

#[test]
fn graph_source_listener_release_retires_real_anonymous_xpc_connections() {
    let initial = FINALIZED.load(Ordering::Acquire);
    for _ in 0..32 {
        drop(Dispatch::with_backend(Fixture).unwrap());
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while FINALIZED.load(Ordering::Acquire) - initial < 32 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(FINALIZED.load(Ordering::Acquire) - initial, 32);
}
