//! Serial ownership of native sessions until unregistration and callback drain.

use super::{CoreAudio, TapError, TapSession};
use std::{
    ffi::{c_char, c_void},
    ptr,
    sync::{Mutex, OnceLock},
};

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attribute: *const c_void) -> *mut c_void;
    fn dispatch_async_f(
        queue: *mut c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_after_f(
        when: u64,
        queue: *mut c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_time(when: u64, delta: i64) -> u64;
}

pub(crate) fn checked_queue(create: impl FnOnce() -> *mut c_void) -> Result<usize, TapError> {
    let queue = create();
    if queue.is_null() {
        Err(TapError::code(6))
    } else {
        Ok(queue as usize)
    }
}

pub(crate) fn prepare_cleanup() -> Result<usize, TapError> {
    static QUEUE: OnceLock<usize> = OnceLock::new();
    static INITIALIZE: Mutex<()> = Mutex::new(());
    if let Some(queue) = QUEUE.get() {
        return Ok(*queue);
    }
    let _guard = INITIALIZE.lock().map_err(|_| TapError::code(9))?;
    if let Some(queue) = QUEUE.get() {
        return Ok(*queue);
    }
    // SAFETY: static NUL-terminated label; successful serial queue is process-lived.
    let queue = checked_queue(|| unsafe {
        dispatch_queue_create(c"io.mixrelay.capture.retired".as_ptr(), ptr::null())
    })?;
    // The initialization lock admits one publisher, so this stores `queue`.
    Ok(*QUEUE.get_or_init(|| queue))
}

pub(crate) fn retire(session: *mut TapSession, retry: bool) {
    // SAFETY: creator acquired this non-null queue before any native publication;
    // queue outlives every admitted session and the retiring owner remains live.
    let queue = unsafe { &*session }.retirement_queue;
    // SAFETY: this queue exclusively owns the live session until cleanup/retry completes.
    unsafe { enqueue(queue, session.cast(), cleanup, retry) };
}

// SAFETY: queue is a successfully created process-lived serial queue. Context
// remains allocated until work consumes it or explicitly schedules its next retry.
pub(crate) unsafe fn enqueue(
    queue: usize,
    context: *mut c_void,
    work: unsafe extern "C" fn(*mut c_void),
    retry: bool,
) {
    // SAFETY: destroy transfers exclusive retirement responsibility to this queue.
    // On failure the same owner is requeued, never copied or freed prematurely.
    unsafe {
        if retry {
            dispatch_after_f(
                dispatch_time(0, 1_000_000_000),
                queue as *mut c_void,
                context,
                work,
            );
        } else {
            dispatch_async_f(queue as *mut c_void, context, work);
        }
    }
}

unsafe extern "C" fn cleanup(context: *mut c_void) {
    let pointer = context.cast::<TapSession>();
    // SAFETY: single serial queue owns the allocation until reclaim returns true.
    if unsafe { &*pointer }.reclaim(&CoreAudio) {
        // SAFETY: every native handle removed and callbacks/listeners drained; caller
        // transferred pump/observer ownership before asynchronous destroy.
        drop(unsafe { Box::from_raw(pointer) });
    } else {
        retire(pointer, true);
    }
}
