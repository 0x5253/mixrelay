//! One control-plane wake capability; audio callbacks never invoke it.
use std::ffi::c_void;

pub(crate) struct ControlSignal {
    context: *const c_void,
    invoke: unsafe extern "C" fn(*const c_void),
}
// SAFETY: construction requires a process-lifetime context and a thread-safe
// enqueue callback. Only that callback touches context; it never runs PCM work inline.
unsafe impl Send for ControlSignal {}
unsafe impl Sync for ControlSignal {}
impl ControlSignal {
    /// # Safety
    /// Context outlives the registry and invoke may run concurrently on any
    /// control caller. It only enqueues work, without borrowing control state inline.
    pub(crate) unsafe fn new(
        context: *const c_void,
        invoke: unsafe extern "C" fn(*const c_void),
    ) -> Self {
        Self { context, invoke }
    }
    pub(crate) fn notify(&self) {
        // SAFETY: the constructor's retained capability permits concurrent control notifications.
        unsafe { (self.invoke)(self.context) };
    }
}
