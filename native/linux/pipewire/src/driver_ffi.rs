//! C diagnostic probes use the same owned driver, not a separate C implementation.
use crate::{DriverCycle, DriverFormat, DriverProcess, GraphDriver};
use std::ffi::c_void;

pub(crate) struct Probe {
    process: unsafe extern "C" fn(*mut c_void, *const DriverCycle),
    userdata: *mut c_void,
}

// SAFETY: mr_pw_driver_open requires the C probe to retain its callback context,
// hand its mutable processing role exclusively to this driver, and close/drain
// before reclamation. Other probe observations must use their own atomics.
unsafe impl Send for Probe {}

impl DriverProcess for Probe {
    fn process(&mut self, cycle: &DriverCycle) {
        // SAFETY: the FFI owner retains its context through driver close/drain.
        unsafe { (self.process)(self.userdata, cycle) };
    }
}

/// Opens and starts a complete native driver for a C diagnostic probe.
/// Null indicates failure and no surviving callback. No external connection is used.
/// # Safety
/// `userdata` is retained until close and its processing role belongs exclusively
/// to the serialized callback. The function must be RT-safe and never unwind.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_driver_open(
    process: Option<unsafe extern "C" fn(*mut c_void, *const DriverCycle)>,
    userdata: *mut c_void,
) -> *mut GraphDriver<Probe> {
    let Some(process) = process else {
        return std::ptr::null_mut();
    };
    if userdata.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(format) = DriverFormat::new(48_000, 128) else {
        return std::ptr::null_mut();
    };
    GraphDriver::open(Probe { process, userdata }, format).map_or(std::ptr::null_mut(), |driver| {
        Box::into_raw(Box::new(driver))
    })
}

/// Closes the probe's unique driver and drains callbacks; null is harmless.
/// # Safety
/// The pointer is an unreleased open result; no other control call may overlap.
/// Never call from its callback. Context ownership returns only after this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_pw_driver_close(driver: *mut GraphDriver<Probe>) {
    if !driver.is_null() {
        // SAFETY: the C probe returns this unique allocation exactly once.
        drop(unsafe { Box::from_raw(driver) });
    }
}
