//! Read-only control-plane validation of a borrowed transport descriptor.
//!
//! This module never binds a HAL context or owns a device's active mapping.
//! The Rust HAL runtime alone prepares, commits and retires those mappings.
use crate::endpoint_transport::{
    EndpointTransportConfig, mr_endpoint_transport_attach, mr_endpoint_transport_config,
};
use std::{
    ffi::{c_int, c_void},
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    ptr,
};

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        size: usize,
        protection: c_int,
        flags: c_int,
        descriptor: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> c_int;
    fn fcntl(descriptor: c_int, command: c_int, ...) -> c_int;
}

struct InspectionMapping {
    memory: *mut c_void,
    size: usize,
}

impl InspectionMapping {
    fn open(descriptor: c_int, size: usize) -> io::Result<Self> {
        if descriptor < 0 || size == 0 || u32::try_from(size).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid transport descriptor or size",
            ));
        }
        // F_DUPFD_CLOEXEC rejects stale descriptors without violating the live
        // descriptor precondition of BorrowedFd::borrow_raw. The clone belongs
        // only to this inspection and closes on every success/failure path.
        let duplicate = unsafe { fcntl(descriptor, 67, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = File::from(unsafe { OwnedFd::from_raw_fd(duplicate) });
        if file.metadata()?.len() < size as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "transport backing is undersized",
            ));
        }
        // Read/write mapping permission is validated because the actual HAL
        // runtime must publish PCM and clock metadata. Inspection does not write.
        let memory = unsafe { mmap(ptr::null_mut(), size, 3, 1, file.as_raw_fd(), 0) };
        if memory == ptr::without_provenance_mut(usize::MAX) {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { memory, size })
    }

    fn config(&self) -> io::Result<EndpointTransportConfig> {
        let transport = unsafe { mr_endpoint_transport_attach(self.memory, self.size) };
        let mut config = EndpointTransportConfig::default();
        if transport.is_null()
            || !unsafe { mr_endpoint_transport_config(transport, &raw mut config) }
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid transport layout",
            ));
        }
        Ok(config)
    }
}

impl Drop for InspectionMapping {
    fn drop(&mut self) {
        unsafe { munmap(self.memory, self.size) };
    }
}

/// Validates the current transport layout without changing its clock or binding.
/// The caller retains its descriptor; the inspection view is always unmapped.
///
/// # Errors
/// Returns an error for invalid/stale descriptors, insufficient backing size or
/// access rights, mapping failure, or an invalid current transport layout.
pub fn inspect_descriptor(descriptor: c_int, size: usize) -> io::Result<EndpointTransportConfig> {
    InspectionMapping::open(descriptor, size)?.config()
}

#[cfg(test)]
#[path = "unit_tests/transport_inspection.rs"]
mod tests;
