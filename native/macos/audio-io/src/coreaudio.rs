use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::NonNull;

use mixrelay_audio_io::{AudioDeviceDescriptor, HostAudioBackendError};

use mixrelay_audio_io::discovery::{DeviceInventory, DeviceObservation};

const SYSTEM: u32 = 1;
const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const UTF8: u32 = 0x0800_0100;
const MAX_DEVICE_BYTES: u32 = 1024 * 4;
const MAX_PROPERTY_BYTES: u32 = 65_536;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

impl Address {
    fn new(selector: [u8; 4], scope: u32) -> Self {
        Self {
            selector: u32::from_be_bytes(selector),
            scope,
            element: 0,
        }
    }
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        bytes: *const c_char,
        encoding: u32,
    ) -> *mut c_void;
    fn CFRelease(value: *const c_void);
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(value: *const c_void) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        value: *const c_void,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> bool;
}

pub(super) struct CoreAudioReader;

impl DeviceInventory for CoreAudioReader {
    type Id = u32;
    fn resolve_uid(&self, uid: &str) -> Result<Option<u32>, HostAudioBackendError> {
        let bytes = CString::new(uid).map_err(|error| failure(error.to_string()))?;
        if bytes.is_empty() {
            return Err(failure("empty CoreAudio device UID".into()));
        }
        // SAFETY: CF copies a valid NUL-terminated UTF8 string. OwnedString adopts
        // its create-rule reference and keeps it alive through the synchronous query.
        let string = OwnedString(
            NonNull::new(unsafe {
                CFStringCreateWithCString(std::ptr::null(), bytes.as_ptr(), UTF8)
            })
            .ok_or_else(|| failure("CoreAudio UID allocation failed".into()))?,
        );
        let pointer = string.0.as_ptr();
        let qualifier_size = u32::try_from(std::mem::size_of_val(&pointer))
            .map_err(|error| failure(error.to_string()))?;
        let address = Address::new(*b"uidd", GLOBAL);
        let mut device = 0_u32;
        let mut size = 4;
        // SAFETY: qualifier is one retained CFStringRef; output is exactly one
        // aligned AudioDeviceID. Neither address escapes this synchronous call.
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM,
                &raw const address,
                qualifier_size,
                (&raw const pointer).cast(),
                &raw mut size,
                (&raw mut device).cast(),
            )
        };
        if status != 0 || size != 4 {
            return Err(failure(format!(
                "CoreAudio UID lookup failed ({status}, {size} bytes)"
            )));
        }
        Ok((device != 0).then_some(device))
    }

    fn device_ids(&self) -> Result<Vec<u32>, HostAudioBackendError> {
        let bytes = property_bytes(SYSTEM, &Address::new(*b"dev#", GLOBAL), MAX_DEVICE_BYTES)?;
        decode_device_ids(bytes.as_bytes())
    }

    fn default_input(&self) -> Option<u32> {
        scalar(SYSTEM, *b"dIn ").filter(|id| *id != 0)
    }

    fn default_output(&self) -> Option<u32> {
        scalar(SYSTEM, *b"dOut").filter(|id| *id != 0)
    }

    fn device(&self, id: u32) -> Option<DeviceObservation> {
        // A dead device must not trigger stream geometry RPCs during recovery.
        let alive = scalar(id, *b"livn").unwrap_or(1) != 0;
        if !alive {
            return None;
        }
        Some(DeviceObservation {
            descriptor: AudioDeviceDescriptor {
                uid: string(id, *b"uid ")?,
                display_name: string(id, *b"lnam")?,
                input_channels: channels(id, *b"inpt"),
                output_channels: channels(id, *b"outp"),
                is_default_input: false,
                is_default_output: false,
            },
            alive,
            hidden: scalar(id, *b"hidn").unwrap_or(0) != 0,
        })
    }
}

fn failure(message: String) -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure { message }
}

fn scalar(object: u32, selector: [u8; 4]) -> Option<u32> {
    let address = Address::new(selector, GLOBAL);
    let mut value = 0_u32;
    let mut size = 4;
    // SAFETY: these fixed scalar selectors return UInt32. Stack storage is
    // aligned, writable and exclusive for this synchronous CoreAudio call;
    // no qualifier pointers or output addresses escape the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
            (&raw mut value).cast(),
        )
    };
    (status == 0 && size == 4).then_some(value)
}

struct PropertyBytes {
    words: Vec<u64>,
    length: usize,
}

impl PropertyBytes {
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: words owns initialized contiguous storage aligned for every
        // CoreAudio property read here. length was checked against its allocated
        // byte size after the synchronous write; this shared borrow prevents
        // mutation or deallocation until the returned byte slice is released.
        unsafe { std::slice::from_raw_parts(self.words.as_ptr().cast(), self.length) }
    }
}

fn property_bytes(
    object: u32,
    address: &Address,
    maximum: u32,
) -> Result<PropertyBytes, HostAudioBackendError> {
    let mut size = 0;
    // SAFETY: address and size are live aligned stack/borrowed values; CoreAudio
    // synchronously writes only one UInt32 size and retains neither pointer.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(object, address, 0, std::ptr::null(), &raw mut size)
    };
    if status != 0 || size > maximum {
        return Err(failure(format!(
            "CoreAudio property size failed ({status}, {size} bytes)"
        )));
    }
    read_property_bytes(size, maximum, |words, size| {
        // SAFETY: the bounded reader owns aligned initialized words covering
        // *size bytes. CoreAudio receives exactly that exclusive output extent.
        unsafe {
            AudioObjectGetPropertyData(
                object,
                address,
                0,
                std::ptr::null(),
                size,
                words.as_mut_ptr().cast(),
            )
        }
    })
}

fn read_property_bytes(
    mut size: u32,
    maximum: u32,
    read: impl FnOnce(&mut [u64], &mut u32) -> i32,
) -> Result<PropertyBytes, HostAudioBackendError> {
    if size > maximum {
        return Err(failure(format!(
            "CoreAudio property exceeds {maximum} bytes"
        )));
    }
    let mut words = vec![0_u64; (size as usize).div_ceil(8)];
    let capacity = size;
    if size != 0 {
        let status = read(&mut words, &mut size);
        if status != 0 || size > capacity {
            return Err(failure(format!(
                "CoreAudio property read failed ({status}, {size} bytes)"
            )));
        }
    }
    Ok(PropertyBytes {
        words,
        length: size as usize,
    })
}

fn decode_device_ids(bytes: &[u8]) -> Result<Vec<u32>, HostAudioBackendError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(failure("CoreAudio returned a truncated device list".into()));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect())
}

fn channels(object: u32, scope: [u8; 4]) -> u32 {
    property_bytes(
        object,
        &Address::new(*b"slay", u32::from_be_bytes(scope)),
        MAX_PROPERTY_BYTES,
    )
    .ok()
    .and_then(|bytes| channel_count(bytes.as_bytes()))
    .unwrap_or(0)
}

fn channel_count(bytes: &[u8]) -> Option<u32> {
    // Both supported macOS ABIs place AudioBufferList.mBuffers at byte8 and use
    // 16-byte AudioBuffers. We read only channel scalars, never their OS pointers.
    let count = u32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let buffers = bytes.get(8..8_usize.checked_add(count.checked_mul(16)?)?)?;
    Some(buffers.chunks_exact(16).fold(0_u32, |sum, bytes| {
        sum.saturating_add(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }))
}

struct OwnedString(NonNull<c_void>);

impl Drop for OwnedString {
    fn drop(&mut self) {
        // SAFETY: string() acquires exactly one retained CFString reference from
        // CoreAudio's UID/name property contract. This owner never clones or
        // transfers the pointer and releases that reference once, on every exit.
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

impl OwnedString {
    fn text(&self) -> Option<String> {
        // SAFETY: this owner retains a live CF object; a mistyped property is
        // rejected before invoking any string-specific operations on it.
        if unsafe { CFGetTypeID(self.0.as_ptr()) != CFStringGetTypeID() } {
            return None;
        }
        // SAFETY: this owner keeps its immutable CFString retained throughout
        // the length/conversion calls. The returned UTF8 bytes have Rust ownership.
        let maximum =
            unsafe { CFStringGetMaximumSizeForEncoding(CFStringGetLength(self.0.as_ptr()), UTF8) };
        let size = usize::try_from(maximum).ok()?.checked_add(1)?;
        if size > MAX_PROPERTY_BYTES as usize {
            return None;
        }
        let mut bytes = vec![0_u8; size];
        // SAFETY: `bytes` is exclusively owned writable storage of the supplied
        // byte size; CoreFoundation writes a NUL-terminated UTF8 representation
        // synchronously and does not retain the buffer.
        if !unsafe {
            CFStringGetCString(
                self.0.as_ptr(),
                bytes.as_mut_ptr().cast(),
                isize::try_from(size).ok()?,
                UTF8,
            )
        } {
            return None;
        }
        CStr::from_bytes_until_nul(&bytes)
            .ok()?
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

fn string(object: u32, selector: [u8; 4]) -> Option<String> {
    let address = Address::new(selector, GLOBAL);
    // SAFETY: only UID/name selectors enter here. Their non-null outputs are
    // retained CFString references, and the closure does not retain its outputs.
    unsafe {
        read_string(|value, size| {
            AudioObjectGetPropertyData(
                object,
                &raw const address,
                0,
                std::ptr::null(),
                size,
                std::ptr::from_mut(value).cast(),
            )
        })
    }
}

// SAFETY: a non-null pointer written by read transfers one owned, immutable
// CF object reference, including when a read returns malformed size or an error.
// text() verifies its type before any CFString operation.
unsafe fn read_string(read: impl FnOnce(&mut *mut c_void, &mut u32) -> i32) -> Option<String> {
    let mut value = std::ptr::null_mut::<c_void>();
    let mut size = u32::try_from(std::mem::size_of_val(&value)).ok()?;
    let status = read(&mut value, &mut size);
    // Adopt before checking status/extent so partial property failures cannot
    // abandon a retained object already transferred into initialized output.
    let value = OwnedString(NonNull::new(value)?);
    if status != 0 || size as usize != std::mem::size_of::<*mut c_void>() {
        return None;
    }
    value.text()
}

#[cfg(test)]
#[path = "property_tests.rs"]
mod tests;
