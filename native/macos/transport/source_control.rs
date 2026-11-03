//! Clocked application-source XPC framing. The registry owns producer leases;
//! this codec validates wire data and FD ownership. HAL inventory is separate.
use crate::control_inventory::{
    Message, NAME_BYTES, Xpc, new_message, operation, uint, xpc_dictionary_create_reply,
    xpc_dictionary_dup_fd, xpc_dictionary_set_fd, xpc_dictionary_set_string,
    xpc_dictionary_set_uint64,
};
use std::{
    ffi::{CStr, CString, c_char, c_int},
    os::fd::{FromRawFd, IntoRawFd, OwnedFd},
    ptr,
};

unsafe extern "C" {
    fn xpc_dictionary_get_string(message: Xpc, key: *const c_char) -> *const c_char;
}

#[repr(C)]
pub struct SourceAttachment {
    pub descriptor: c_int,
    pub mapped_size: usize,
    pub producer_lease: u64,
    pub source_epoch: u64,
}
impl Default for SourceAttachment {
    fn default() -> Self {
        Self {
            descriptor: -1,
            mapped_size: 0,
            producer_lease: 0,
            source_epoch: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRequest {
    pub source_id: [c_char; NAME_BYTES],
    pub capacity_frames: u32,
    pub channels: u32,
    pub transport_version: u32,
}
impl Default for SourceRequest {
    fn default() -> Self {
        Self {
            source_id: [0; NAME_BYTES],
            capacity_frames: 0,
            channels: 0,
            transport_version: 0,
        }
    }
}

// A bounded byte scan never requires short caller strings to occupy 128 bytes.
unsafe fn source_id(pointer: *const c_char) -> Option<CString> {
    if pointer.is_null() {
        return None;
    }
    let mut bytes = Vec::with_capacity(NAME_BYTES);
    for index in 0..NAME_BYTES {
        let byte = unsafe { pointer.add(index).read() }.to_ne_bytes()[0];
        if byte == 0 {
            if bytes.is_empty() || std::str::from_utf8(&bytes).is_err() {
                return None;
            }
            return CString::new(bytes).ok();
        }
        bytes.push(byte);
    }
    None
}
fn positive_u32(message: Xpc, key: &CStr) -> Option<u32> {
    u32::try_from(uint(message, key)?)
        .ok()
        .filter(|value| *value != 0)
}
fn parse_request(message: Xpc) -> Option<SourceRequest> {
    if !operation(message, c"request_source_transport")
        || uint(message, c"source_transport_version")
            != Some(u64::from(crate::application_clock::VERSION))
    {
        return None;
    }
    let source = unsafe { source_id(xpc_dictionary_get_string(message, c"source_id".as_ptr())) }?;
    let mut parsed = SourceRequest {
        capacity_frames: positive_u32(message, c"capacity_frames")?,
        channels: positive_u32(message, c"channels")?,
        transport_version: crate::application_clock::VERSION,
        ..SourceRequest::default()
    };
    for (target, byte) in parsed.source_id.iter_mut().zip(source.as_bytes()) {
        *target = c_char::from_ne_bytes([*byte]);
    }
    Some(parsed)
}

/// # Safety
/// Source ID is null or readable through its first NUL, at most 128 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_control_create_clocked_source_request(
    source: *const c_char,
    capacity_frames: u32,
    channels: u32,
) -> Xpc {
    let Some(source) = (unsafe { source_id(source) }) else {
        return ptr::null_mut();
    };
    if capacity_frames == 0 || channels == 0 {
        return ptr::null_mut();
    }
    let Some(message) = new_message(c"request_source_transport") else {
        return ptr::null_mut();
    };
    unsafe {
        xpc_dictionary_set_string(message.0, c"source_id".as_ptr(), source.as_ptr());
        xpc_dictionary_set_uint64(
            message.0,
            c"capacity_frames".as_ptr(),
            u64::from(capacity_frames),
        );
        xpc_dictionary_set_uint64(message.0, c"channels".as_ptr(), u64::from(channels));
        xpc_dictionary_set_uint64(
            message.0,
            c"source_transport_version".as_ptr(),
            u64::from(crate::application_clock::VERSION),
        );
    }
    message.into_raw()
}

/// # Safety
/// Message is null or live XPC. Output is writable. Malformed requests leave
/// output unchanged; only the current clocked transport version is accepted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_control_parse_source_request(
    message: Xpc,
    request: *mut SourceRequest,
) -> bool {
    if request.is_null() {
        return false;
    }
    let Some(parsed) = parse_request(message) else {
        return false;
    };
    unsafe { request.write(parsed) };
    true
}

fn populate_attachment(
    message: Message,
    descriptor: c_int,
    size: usize,
    lease: u64,
    epoch: u64,
) -> Xpc {
    if descriptor < 0 || size == 0 || u32::try_from(size).is_err() || lease == 0 || epoch == 0 {
        return ptr::null_mut();
    }
    unsafe {
        xpc_dictionary_set_uint64(message.0, c"version".as_ptr(), 1);
        xpc_dictionary_set_string(
            message.0,
            c"operation".as_ptr(),
            c"attach_source_transport".as_ptr(),
        );
        xpc_dictionary_set_uint64(
            message.0,
            c"source_transport_version".as_ptr(),
            u64::from(crate::application_clock::VERSION),
        );
        xpc_dictionary_set_fd(message.0, c"transport_fd".as_ptr(), descriptor);
        xpc_dictionary_set_uint64(message.0, c"mapped_size".as_ptr(), size as u64);
        xpc_dictionary_set_uint64(message.0, c"producer_lease".as_ptr(), lease);
        xpc_dictionary_set_uint64(message.0, c"source_epoch".as_ptr(), epoch);
    }
    message.into_raw()
}

/// # Safety
/// Request is null or live XPC. Descriptor is live and borrowed; XPC retains a
/// duplicate. All required lease metadata is published in the same reply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_control_create_source_attachment_reply(
    request: Xpc,
    descriptor: c_int,
    mapped_size: usize,
    producer_lease: u64,
    source_epoch: u64,
) -> Xpc {
    if parse_request(request).is_none()
        || descriptor < 0
        || mapped_size == 0
        || u32::try_from(mapped_size).is_err()
        || producer_lease == 0
        || source_epoch == 0
    {
        return ptr::null_mut();
    }
    Message::new(unsafe { xpc_dictionary_create_reply(request) }).map_or(
        ptr::null_mut(),
        |message| {
            populate_attachment(
                message,
                descriptor,
                mapped_size,
                producer_lease,
                source_epoch,
            )
        },
    )
}

/// # Safety
/// Message is null or live XPC. Output is empty/previously closed writable
/// storage. Success transfers one descriptor; errors leave output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_control_parse_source_attachment(
    message: Xpc,
    attachment: *mut SourceAttachment,
) -> bool {
    if attachment.is_null()
        || !operation(message, c"attach_source_transport")
        || uint(message, c"source_transport_version")
            != Some(u64::from(crate::application_clock::VERSION))
    {
        return false;
    }
    let Some(mapped_size) =
        positive_u32(message, c"mapped_size").and_then(|size| usize::try_from(size).ok())
    else {
        return false;
    };
    let Some(producer_lease) = uint(message, c"producer_lease").filter(|value| *value != 0) else {
        return false;
    };
    let Some(source_epoch) = uint(message, c"source_epoch").filter(|value| *value != 0) else {
        return false;
    };
    let descriptor = unsafe { xpc_dictionary_dup_fd(message, c"transport_fd".as_ptr()) };
    if descriptor < 0 {
        return false;
    }
    let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
    unsafe {
        attachment.write(SourceAttachment {
            descriptor: owned.into_raw_fd(),
            mapped_size,
            producer_lease,
            source_epoch,
        });
    };
    true
}

/// # Safety
/// Non-null attachment contains a uniquely owned parsed FD or an empty value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_control_source_attachment_close(
    attachment: *mut SourceAttachment,
) {
    let Some(attachment) = (unsafe { attachment.as_mut() }) else {
        return;
    };
    if attachment.descriptor >= 0 {
        drop(unsafe { OwnedFd::from_raw_fd(attachment.descriptor) });
    }
    *attachment = SourceAttachment::default();
}

#[cfg(test)]
#[path = "unit_tests/source_control.rs"]
mod tests;
