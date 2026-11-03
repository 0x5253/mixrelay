use std::{
    ffi::{c_int, c_void},
    io, ptr,
};

#[cfg(target_os = "macos")]
const ANONYMOUS: c_int = 0x1000;
#[cfg(target_os = "linux")]
const ANONYMOUS: c_int = 0x20;
const READ_WRITE: c_int = 1 | 2;
const PRIVATE: c_int = 2;

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        descriptor: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
    fn getpagesize() -> c_int;
}

/// Touches every PCM page on control before handing the buffer to audio callbacks.
/// Preserves sample bits, allocates nothing, and does not pin pages against later
/// memory pressure. The exclusive borrow must precede producer/consumer publication.
pub fn prefault_pcm(samples: &mut [f32]) {
    // SAFETY: getpagesize has no arguments or borrowed state on supported Unix targets.
    let page_bytes = usize::try_from(unsafe { getpagesize() }).unwrap_or(4096);
    let step = (page_bytes / size_of::<f32>()).max(1);
    for sample in samples.iter_mut().step_by(step) {
        // SAFETY: the exclusive sample is initialized and writable. Volatile
        // writes force private writable pages even when all samples are zero.
        unsafe { ptr::write_volatile(sample, ptr::read_volatile(sample)) };
    }
    // A heap slice need not begin on a page boundary; include its final page.
    if let Some(sample) = samples.last_mut() {
        // SAFETY: same exclusive initialized sample contract as above.
        unsafe { ptr::write_volatile(sample, ptr::read_volatile(sample)) };
    }
}

/// Private anonymous PCM pages. A zero value owns no mapping and is safe to drop
/// during a native owner's partially initialized cleanup.
#[repr(C)]
#[derive(Debug, Default)]
pub(super) struct Storage {
    pub(super) samples: *mut f32,
    bytes: usize,
}

impl Storage {
    pub(super) fn new(samples: usize) -> io::Result<Self> {
        // SAFETY: the system creates a fresh private mapping or returns MAP_FAILED.
        unsafe {
            Self::map_with(samples, |bytes| {
                mmap(
                    ptr::null_mut(),
                    bytes,
                    READ_WRITE,
                    PRIVATE | ANONYMOUS,
                    -1,
                    0,
                )
            })
        }
    }

    // SAFETY: map returns MAP_FAILED or an exclusively owned anonymous mapping
    // of precisely the requested bytes. Injection remains private to Rust tests.
    pub(super) unsafe fn map_with(
        samples: usize,
        map: impl FnOnce(usize) -> *mut c_void,
    ) -> io::Result<Self> {
        let bytes = samples
            .checked_mul(size_of::<f32>())
            .filter(|&bytes| bytes > 0 && isize::try_from(bytes).is_ok())
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let memory = map(bytes);
        if memory.addr() == usize::MAX {
            return Err(io::Error::last_os_error());
        }
        let storage = Self {
            samples: memory.cast(),
            bytes,
        };
        // SAFETY: this fresh exclusive anonymous mapping is zero-initialized
        // and contains exactly samples floats; no callback can access it yet.
        prefault_pcm(unsafe { std::slice::from_raw_parts_mut(storage.samples, samples) });
        Ok(storage)
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        if !self.samples.is_null() {
            // SAFETY: this control-side owner retains the original mapping extent;
            // clear runs only after producer, consumer and observers have drained.
            let _ = unsafe { munmap(self.samples.cast(), self.bytes) };
        }
    }
}
