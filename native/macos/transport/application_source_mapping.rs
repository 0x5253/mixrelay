use crate::{
    application_clock::{
        ApplicationClock, mr_application_clock_attach, mr_application_clock_initialize,
        mr_application_clock_required_bytes,
    },
    create_shared_memory,
    shared_audio_buffer::{
        SharedAudioBuffer, mr_shared_audio_buffer_attach, mr_shared_audio_buffer_capacity,
        mr_shared_audio_buffer_channels, mr_shared_audio_buffer_initialize,
        mr_shared_audio_buffer_is_lock_free,
    },
};
use std::{
    ffi::c_void,
    io,
    os::fd::{AsRawFd, OwnedFd},
    ptr::NonNull,
};

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}

struct MappedMemory {
    memory: NonNull<c_void>,
    bytes: usize,
}

/// Shared source storage owned by a registry entry or one attached producer.
pub(crate) struct Mapping {
    _memory: MappedMemory,
    pub bytes: usize,
    pub descriptor: OwnedFd,
    pub buffer: NonNull<SharedAudioBuffer>,
    pub clock: NonNull<ApplicationClock>,
}
// SAFETY: the mapping's geometry is immutable. Access after publication uses
// only atomic transport APIs; registry readers pin their owner until reclamation,
// while an attached producer is owned by one pump. Neither lends mutable mappings.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Mapping {
    pub(crate) fn capacity(&self) -> u32 {
        // SAFETY: the retained mapping has immutable validated geometry.
        unsafe { mr_shared_audio_buffer_capacity(self.buffer.as_ptr()) }
    }
    pub(crate) fn new(capacity: u32, channels: u32) -> io::Result<Self> {
        Self::with_storage(capacity, channels, |bytes| {
            create_shared_memory(bytes, true)
        })
    }
    pub(crate) fn with_storage(
        capacity: u32,
        channels: u32,
        storage: impl FnOnce(usize) -> io::Result<OwnedFd>,
    ) -> io::Result<Self> {
        let bytes = mr_application_clock_required_bytes(capacity, channels);
        if bytes == 0 {
            return Err(io::Error::other("invalid source geometry"));
        }
        let descriptor = storage(bytes)?;
        let memory = MappedMemory::new(&descriptor, bytes)?;
        let address = memory.memory.as_ptr();
        let buffer = NonNull::new(unsafe {
            mr_shared_audio_buffer_initialize(address, bytes, capacity, channels)
        });
        let clock = NonNull::new(unsafe {
            mr_application_clock_initialize(address, bytes, capacity, channels)
        });
        let (Some(buffer), Some(clock)) = (buffer, clock) else {
            return Err(io::Error::other("source initialization failed"));
        };
        Ok(Self {
            _memory: memory,
            bytes,
            descriptor,
            buffer,
            clock,
        })
    }
    /// Attaches a descriptor received from the registry without initializing it.
    /// Geometry is checked before exposing the atomically mutable contents.
    /// # Safety
    /// The source registry freezes headers before publishing this descriptor and
    /// never truncates the backing object. Other participants mutate only the
    /// transport's atomic payloads for the entire attached mapping lifetime.
    pub(crate) unsafe fn attach(
        descriptor: OwnedFd,
        bytes: usize,
        capacity: u32,
        channels: u32,
    ) -> io::Result<Self> {
        let required = mr_application_clock_required_bytes(capacity, channels);
        if required == 0 || bytes != required {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source extent mismatch",
            ));
        }
        // A short backing object could otherwise SIGBUS on header access even
        // when mmap succeeds. The registry never truncates a published object.
        let file = std::fs::File::from(descriptor);
        if file.metadata()?.len() < bytes as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "short source storage",
            ));
        }
        let descriptor = OwnedFd::from(file);
        let memory = MappedMemory::new(&descriptor, bytes)?;
        let address = memory.memory.as_ptr();
        // SAFETY: the complete extent is mapped; the source protocol freezes
        // headers before descriptor publication and mutates only atomic payloads.
        let buffer = NonNull::new(unsafe { mr_shared_audio_buffer_attach(address, bytes) });
        let clock = NonNull::new(unsafe {
            mr_application_clock_attach(address, bytes, capacity, channels)
        });
        let (Some(buffer), Some(clock)) = (buffer, clock) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid source headers",
            ));
        };
        // SAFETY: attach validated immutable header layout within this owned mapping.
        if unsafe {
            mr_shared_audio_buffer_channels(buffer.as_ptr()) != channels
                || mr_shared_audio_buffer_capacity(buffer.as_ptr()) != capacity
                || !mr_shared_audio_buffer_is_lock_free(buffer.as_ptr())
        } {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source geometry mismatch",
            ));
        }
        Ok(Self {
            _memory: memory,
            bytes,
            descriptor,
            buffer,
            clock,
        })
    }

    pub(crate) fn clock(&self) -> &ApplicationClock {
        // SAFETY: immutable initialized clock geometry and atomic payload live
        // for this mapping owner's lifetime; no mutable Rust reference exists.
        unsafe { self.clock.as_ref() }
    }
}
impl MappedMemory {
    fn new(descriptor: &OwnedFd, bytes: usize) -> io::Result<Self> {
        // SAFETY: a live descriptor backs this exact RW shared extent. No fixed
        // address is requested; the resulting independent mapping is owned here.
        let address = unsafe { mmap(std::ptr::null_mut(), bytes, 3, 1, descriptor.as_raw_fd(), 0) };
        if address == std::ptr::without_provenance_mut(usize::MAX) {
            return Err(io::Error::last_os_error());
        }
        let Some(memory) = NonNull::new(address) else {
            // SAFETY: successful mmap returned exactly this extent at address zero.
            unsafe { munmap(address, bytes) };
            return Err(io::Error::other("zero address mapping"));
        };
        Ok(Self { memory, bytes })
    }
}

impl Drop for MappedMemory {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the successful mmap extent. All local
        // users drain before its enclosing Mapping is dropped, including failures.
        unsafe {
            munmap(self.memory.as_ptr(), self.bytes);
        }
        // The descriptor closes after unmapping. Remote duplicate descriptors
        // and mappings continue owning their independent kernel references.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_extent_is_rejected_before_mapping_or_header_access() {
        let bytes = mr_application_clock_required_bytes(1024, 2);
        // The current trailer requires its complete extent before header access.
        let descriptor = std::fs::File::open("/dev/null").unwrap().into();
        let result = unsafe { Mapping::attach(descriptor, bytes - 8, 1024, 2) };
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidData);
    }
}
