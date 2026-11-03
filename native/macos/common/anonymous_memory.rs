//! File-free, FD-passable shared memory for macOS transport.
//!
//! macOS has no `memfd_create`/`SHM_ANON` descriptor API. A POSIX shared-memory
//! object is created exclusively with a random kernel namespace name, and that
//! name is removed before the descriptor leaves this module. This never creates
//! a filesystem file; after unlink the object exists only while FDs/mappings own
//! it. No temporary-file fallback is permitted.

use std::{
    ffi::{c_char, c_int, c_void},
    io,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
};

const O_RDWR: c_int = 2;
const O_RDONLY: c_int = 0;
const O_CREAT: c_int = 0x0200;
const O_EXCL: c_int = 0x0800;
const F_GETFD: c_int = 1;
const F_SETFD: c_int = 2;
const FD_CLOEXEC: c_int = 1;
const EEXIST: c_int = 17;
const HEX: &[u8; 16] = b"0123456789abcdef";

unsafe extern "C" {
    fn arc4random_buf(buffer: *mut c_void, size: usize);
    fn shm_open(name: *const c_char, flags: c_int, ...) -> c_int;
    fn shm_unlink(name: *const c_char) -> c_int;
    fn ftruncate(descriptor: c_int, size: i64) -> c_int;
    fn fcntl(descriptor: c_int, command: c_int, ...) -> c_int;
}

struct NamespaceName {
    bytes: [c_char; 29],
    linked: bool,
}

impl NamespaceName {
    fn random() -> Self {
        let mut random = [0_u8; 12];
        // SAFETY: the system RNG receives writable storage of precisely this size.
        unsafe { arc4random_buf(random.as_mut_ptr().cast(), random.len()) };
        let mut bytes = [0; 29];
        for (slot, byte) in bytes.iter_mut().zip(b"/tn-") {
            *slot = c_char::from_ne_bytes([*byte]);
        }
        for (index, byte) in random.into_iter().enumerate() {
            bytes[4 + index * 2] = c_char::from_ne_bytes([HEX[usize::from(byte >> 4)]]);
            bytes[5 + index * 2] = c_char::from_ne_bytes([HEX[usize::from(byte & 15)]]);
        }
        Self {
            bytes,
            linked: false,
        }
    }

    fn unlink(&mut self) -> io::Result<()> {
        // SAFETY: the fixed array contains a NUL-terminated name owned by this call.
        if unsafe { shm_unlink(self.bytes.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.linked = false;
        Ok(())
    }
}

impl Drop for NamespaceName {
    fn drop(&mut self) {
        if self.linked {
            // A failed size/reopen/unlink operation must not intentionally leave
            // a named object behind. Preserve the original error at the caller.
            let _ = unsafe { shm_unlink(self.bytes.as_ptr()) };
        }
    }
}

fn owned_descriptor(descriptor: c_int) -> io::Result<OwnedFd> {
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful shm_open returned a new uniquely owned descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
    // POSIX shm_open sets close-on-exec; verify and enforce it, never leak the
    // transport into a later vendor-process exec. XPC FD duplication is unaffected.
    let flags = unsafe { fcntl(owned.as_raw_fd(), F_GETFD) };
    if flags < 0 || unsafe { fcntl(owned.as_raw_fd(), F_SETFD, flags | FD_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(owned)
}

/// Creates a file-free descriptor owned by the caller; no namespace remains.
/// # Errors
/// Rejects invalid sizes and propagates allocation, sizing or unlink errors.
pub fn create(size: usize, writable: bool) -> io::Result<OwnedFd> {
    let size = i64::try_from(size)
        .ok()
        .filter(|size| *size > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid shared-memory size"))?;
    for _ in 0..8 {
        let mut name = NamespaceName::random();
        // User-only permissions and exclusive creation prevent name takeover.
        let descriptor =
            unsafe { shm_open(name.bytes.as_ptr(), O_RDWR | O_CREAT | O_EXCL, 0o600_u32) };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(EEXIST) {
                continue;
            }
            return Err(error);
        }
        name.linked = true;
        let writer = owned_descriptor(descriptor)?;
        if writable {
            // The production path removes its namespace entry immediately, even
            // before resizing. Size failure then releases an already-anonymous FD.
            name.unlink()?;
            if unsafe { ftruncate(writer.as_raw_fd(), size) } != 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(writer);
        }
        // Native permission tests also need a genuinely read-only FD. Reopen the
        // POSIX object before removing its name; no pathname on disk is involved.
        if unsafe { ftruncate(writer.as_raw_fd(), size) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let reader = owned_descriptor(unsafe { shm_open(name.bytes.as_ptr(), O_RDONLY) })?;
        name.unlink()?;
        return Ok(reader);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "shared-memory name collision limit",
    ))
}

/// Transfers one descriptor to the C owner, which must close it after unmapping.
/// Failure returns -1 and releases every acquired FD/namespace entry.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn mr_shared_memory_create(size: usize, writable: bool) -> c_int {
    create(size, writable).map_or(-1, IntoRawFd::into_raw_fd)
}

#[cfg(test)]
#[path = "unit_tests/anonymous_memory.rs"]
mod tests;
