//! Control-owner registration lifetime; PCM queues and callback state stay separate.
//! A failed unregister retains the exact device and procedure pair for retry.

use std::{ffi::c_void, ptr::NonNull};

#[cfg(test)]
mod tests;

const BAD_DEVICE: i32 = i32::from_be_bytes(*b"!dev");
const BAD_OBJECT: i32 = i32::from_be_bytes(*b"!obj");
const ILLEGAL_OPERATION: i32 = i32::from_be_bytes(*b"nope");

/// Control-only ownership of one native registration; callbacks never access it.
#[derive(Debug, Default)]
pub(super) struct DeviceIoLifecycle {
    procedure: Option<NonNull<c_void>>,
    device: u32,
}

#[derive(Debug)]
pub(crate) struct Created {
    pub(crate) procedure: Option<NonNull<c_void>>,
    pub(crate) status: i32,
}

pub(crate) trait Backend {
    fn prepare(&mut self, expected: u32) -> Result<u32, i32>;
    fn create(&mut self, device: u32) -> Created;
    fn start(&mut self, device: u32, procedure: NonNull<c_void>) -> i32;
    fn silence(&mut self);
    fn stop(&mut self, device: u32, procedure: NonNull<c_void>) -> i32;
    fn destroy(&mut self, device: u32, procedure: NonNull<c_void>) -> i32;
}

impl DeviceIoLifecycle {
    pub(crate) fn connect(&mut self, expected: u32, backend: &mut impl Backend) -> i32 {
        if self.procedure.is_some() {
            return -1;
        }
        let device = match backend.prepare(expected) {
            Ok(device) => device,
            Err(status) => return status,
        };
        let created = backend.create(device);
        // Even an error may have written a native handle. Preserve that partial
        // registration so destruction cannot free its still-borrowed context.
        self.device = device;
        self.procedure = created.procedure;
        if created.status != 0 {
            return created.status;
        }
        let Some(procedure) = self.procedure else {
            return ILLEGAL_OPERATION;
        };
        let status = backend.start(device, procedure);
        if status != 0 {
            self.disconnect(backend);
        }
        status
    }

    pub(crate) fn disconnect(&mut self, backend: &mut impl Backend) -> bool {
        let Some(procedure) = self.procedure else {
            return true;
        };
        // Silence precedes both native calls. Stop is best-effort; only destroy
        // or a confirmed deleted object acknowledges callback drain/unregister.
        backend.silence();
        backend.stop(self.device, procedure);
        match backend.destroy(self.device, procedure) {
            0 | BAD_DEVICE | BAD_OBJECT => {
                self.procedure = None;
                self.device = 0;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn registered(&self) -> bool {
        self.procedure.is_some()
    }
}
