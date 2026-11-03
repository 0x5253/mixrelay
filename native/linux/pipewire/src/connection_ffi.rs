use crate::{Connection, ConnectionHandles};
use std::ffi::{CStr, c_char};

/// Opens the same Rust-owned audio context used by the engine for native C probes.
/// Null indicates preparation failure; no native child has been attached yet.
/// # Safety
/// Non-null name must point to a readable NUL-terminated string for this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_connection_open(name: *const c_char) -> *mut Connection {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: C probe supplies a static, NUL-terminated thread name.
    let name = unsafe { CStr::from_ptr(name) };
    Connection::new(name).map_or(std::ptr::null_mut(), |owner| Box::into_raw(Box::new(owner)))
}

/// Borrows the handles of an owned connection without starting or cloning it.
/// # Safety
/// `owner` is a live pointer returned by open. Its children must close before
/// `mr_pw_connection_close`; control calls use the returned loop's lock after startup.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_connection_handles(
    owner: *const Connection,
) -> ConnectionHandles<'static> {
    // SAFETY: the C owner keeps its box live through all native child use. The
    // unbounded borrow crosses only this C ABI; Rust callers use handles directly.
    unsafe { (&*owner).handles() }
}

/// Starts an owned probe connection; zero succeeds and a negative result fails.
/// # Safety
/// `owner` is uniquely borrowed, live, and no caller holds its control-loop lock.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_connection_start(owner: *mut Connection) -> i32 {
    // SAFETY: the C owner serializes startup and retains its box.
    match unsafe { &mut *owner }.start() {
        Ok(()) => 0,
        Err(crate::ConnectionError::Start(code)) => code,
        Err(_) => -1,
    }
}

/// Prepares one inactive capture's monitor ports on this owned probe connection.
/// # Safety
/// `owner` is a started, exclusively borrowed connection; `node_id` belongs to
/// its retained inactive capture stream. Neither child nor connection may close
/// or be operated concurrently before the bounded control preparation returns.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_connection_prepare_capture(
    owner: *mut Connection,
    node_id: u32,
    rate: u32,
) -> i32 {
    // SAFETY: the C fixture pins and serializes its connection and capture owner.
    match unsafe { &mut *owner }.prepare_capture(node_id, rate) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Closes the connection after its final child has disconnected/drained. Null is harmless.
/// # Safety
/// Non-null owner is an unreleased open result. No child, callback, borrowed ABI
/// handle or other control operation may survive this call. Never call from RT.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_connection_close(owner: *mut Connection) {
    if !owner.is_null() {
        // SAFETY: ownership returns exactly once from the C probe to this Box.
        drop(unsafe { Box::from_raw(owner) });
    }
}
