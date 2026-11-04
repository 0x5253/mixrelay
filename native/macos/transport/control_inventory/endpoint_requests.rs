//! Endpoint removal and transport replacement intents. Generation policy belongs to the
//! Driver service; these only validate and copy the wire fields.
use super::xpc::{
    Xpc, endpoint_id, new_message, operation, uint, xpc_dictionary_set_data,
    xpc_dictionary_set_uint64,
};
use std::ptr;

/// # Safety
/// A non-null ID must point to 16 readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_create_endpoint_remove(
    id: *const u8,
    generation: u64,
) -> Xpc {
    if id.is_null() || generation == 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(id, 16) };
    if bytes.iter().all(|byte| *byte == 0) {
        return ptr::null_mut();
    }
    let Some(message) = new_message(c"remove_endpoint") else {
        return ptr::null_mut();
    };
    unsafe {
        xpc_dictionary_set_data(message.0, c"endpoint_id".as_ptr(), id.cast(), 16);
        xpc_dictionary_set_uint64(message.0, c"generation".as_ptr(), generation);
    }
    message.into_raw()
}

/// # Safety
/// Message must be live XPC; outputs must be writable, distinct storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_parse_endpoint_remove(
    message: Xpc,
    id: *mut u8,
    generation: *mut u64,
) -> bool {
    if id.is_null() || generation.is_null() || !operation(message, c"remove_endpoint") {
        return false;
    }
    let Some(parsed_id) = endpoint_id(message) else {
        return false;
    };
    let Some(parsed_generation) = uint(message, c"generation").filter(|value| *value != 0) else {
        return false;
    };
    unsafe {
        ptr::copy_nonoverlapping(parsed_id.as_ptr(), id, 16);
        generation.write(parsed_generation);
    }
    true
}

/// Creates a replacement intent, not a registry mutation. The Driver service must
/// compare the expected generation before retiring an endpoint's mapping.
///
/// # Safety
/// A non-null ID must point to 16 readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_create_transport_replacement_request(
    id: *const u8,
    expected_generation: u64,
) -> Xpc {
    if id.is_null() || expected_generation == 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(id, 16) };
    if bytes.iter().all(|byte| *byte == 0) {
        return ptr::null_mut();
    }
    let Some(message) = new_message(c"replace_transport") else {
        return ptr::null_mut();
    };
    unsafe {
        xpc_dictionary_set_data(message.0, c"endpoint_id".as_ptr(), id.cast(), 16);
        xpc_dictionary_set_uint64(
            message.0,
            c"expected_generation".as_ptr(),
            expected_generation,
        );
    }
    message.into_raw()
}

/// # Safety
/// Message must be live XPC; outputs must be writable, distinct storage. Invalid
/// messages leave both outputs unchanged. Generation policy belongs to the Driver service.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_parse_transport_replacement_request(
    message: Xpc,
    id: *mut u8,
    expected_generation: *mut u64,
) -> bool {
    if id.is_null() || expected_generation.is_null() || !operation(message, c"replace_transport") {
        return false;
    }
    let Some(parsed_id) = endpoint_id(message) else {
        return false;
    };
    let Some(parsed_generation) = uint(message, c"expected_generation").filter(|value| *value != 0)
    else {
        return false;
    };
    unsafe {
        ptr::copy_nonoverlapping(parsed_id.as_ptr(), id, 16);
        expected_generation.write(parsed_generation);
    }
    true
}
