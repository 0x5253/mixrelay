//! Borrowed `CoreAudio` buffer metadata, shared by native capture and device adapters.
use std::slice;

/// `CoreAudio` `AudioBuffer` layout on supported 64-bit macOS targets.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioBuffer {
    /// Interleaved channel width of this stream, including disabled streams.
    pub channels: u32,
    /// Extent supplied by the native callback, in bytes.
    pub byte_size: u32,
    /// Borrowed native PCM; null denotes a disabled stream.
    pub data: *mut f32,
}

/// Header of `CoreAudio`'s trailing-array `AudioBufferList`; never owns its buffers.
#[repr(C)]
#[derive(Debug)]
pub struct AudioBufferList {
    /// Number of native metadata entries following the header.
    pub count: u32,
    /// The OS allocation contains `count` entries at this aligned offset.
    pub buffers: [AudioBuffer; 0],
}

const _: () = assert!(size_of::<AudioBuffer>() == 16);
const _: () = assert!(std::mem::offset_of!(AudioBufferList, buffers) == 8);

/// Borrows metadata only; empty, missing or excessive lists are rejected.
/// # Safety
/// `CoreAudio` (or a sized fixture) owns the aligned header and its complete trailing
/// array for the returned lifetime. No code may mutate metadata during the borrow.
#[must_use]
pub unsafe fn buffers<'a>(
    list: *const AudioBufferList,
    maximum: usize,
) -> Option<&'a [AudioBuffer]> {
    if list.is_null() {
        return None;
    }
    // SAFETY: retain the original allocation pointer for the trailing array;
    // borrowing only the sized header would not cover the following descriptors.
    let count = unsafe { std::ptr::addr_of!((*list).count).read() } as usize;
    if count == 0 || count > maximum {
        return None;
    }
    // SAFETY: native allocation contains exactly the advertised metadata extent;
    // this creates no references to PCM and imposes no aliasing on other streams.
    Some(unsafe {
        slice::from_raw_parts(
            std::ptr::addr_of!((*list).buffers).cast::<AudioBuffer>(),
            count,
        )
    })
}
