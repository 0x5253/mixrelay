//! The property entry points the C vtable shim forwards to.
use super::{
    Address, INPUT,
    lock_free::TRANSPORT,
    output_controls::convert_level,
    property, settings,
    status::{BAD_SIZE, ILLEGAL},
    value::{UTF8, Value, number},
};
use crate::registry::Registry;
use std::ffi::{c_char, c_void};

const DEVICE_FOR_UID: u32 = u32::from_be_bytes(*b"uidd");
const LATENCY: u32 = u32::from_be_bytes(*b"ltnc");

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetCString(
        string: *const c_void,
        output: *mut c_char,
        capacity: isize,
        encoding: u32,
    ) -> u8;
    fn CFGetTypeID(object: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_has_property(
    registry: *mut Registry,
    object: u32,
    address: *const Address,
) -> bool {
    match (unsafe { registry.as_ref() }, unsafe { address.as_ref() }) {
        (Some(registry), Some(address)) => property(registry, object, address, None).is_ok(),
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_property_size(
    registry: *mut Registry,
    object: u32,
    address: *const Address,
    output: *mut u32,
) -> i32 {
    let (Some(registry), Some(address), Some(output)) = (
        unsafe { registry.as_ref() },
        unsafe { address.as_ref() },
        unsafe { output.as_mut() },
    ) else {
        return ILLEGAL;
    };
    match property(registry, object, address, None) {
        Ok(value) => match u32::try_from(value.size()) {
            Ok(size) => {
                *output = size;
                0
            }
            Err(_) => BAD_SIZE,
        },
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_property_get(
    registry: *mut Registry,
    object: u32,
    address: *const Address,
    qualifier_size: u32,
    qualifier: *const c_void,
    capacity: u32,
    written: *mut u32,
    output: *mut c_void,
    requester_pid: i32,
) -> i32 {
    let (Some(registry), Some(address), Some(written)) = (
        unsafe { registry.as_ref() },
        unsafe { address.as_ref() },
        unsafe { written.as_mut() },
    ) else {
        return ILLEGAL;
    };
    let deadline = registry.transport.deadline(crate::transport::QUERY_BUDGET);
    if output.is_null() && capacity != 0 {
        return ILLEGAL;
    }
    // `uidd` is never answered on an IO thread (see `plugin`); skip its allocating parse.
    let uid = if address.selector == DEVICE_FOR_UID && !crate::transport::on_audio_thread() {
        match unsafe { qualifier_string(qualifier_size, qualifier) } {
            Ok(uid) => uid,
            Err(error) => return error,
        }
    } else {
        None
    };
    match property(registry, object, address, uid.as_deref())
        .map(|value| present_to_requester(registry, address, value, (requester_pid, deadline)))
        .and_then(|value| unsafe { convert_level(value, address, capacity, output) })
        .and_then(|value| unsafe { value.write(capacity as usize, output) })
    {
        Ok(size) => match u32::try_from(size) {
            Ok(size) => {
                *written = size;
                if address.selector == TRANSPORT && size >= 4 {
                    // SAFETY: Value::write has written these four bytes within capacity.
                    let value = unsafe { output.cast::<u32>().read_unaligned() };
                    crate::transport::report_presentation(requester_pid, value);
                }
                0
            }
            Err(_) => BAD_SIZE,
        },
        Err(error) => error,
    }
}

/// Answers that depend on who asks: the transport type each process sees,
/// and the input latency `coreaudiod` itself must not apply.
fn present_to_requester(
    registry: &Registry,
    address: &Address,
    value: Value,
    (requester_pid, deadline): (i32, mixrelay_time::Instant),
) -> Value {
    if address.selector == TRANSPORT {
        number(registry.transport.get_until(requester_pid, deadline))
    } else if address.selector == LATENCY
        && address.scope == INPUT
        && registry.transport.is_coreaudio(requester_pid)
    {
        // Even a one-frame marker changes the host's input coordinates.
        // App content-delay updates notify without changing host scheduling.
        number(0)
    } else {
        value
    }
}

/// Reads the `CFString` UID qualifier of a device-for-UID lookup.
unsafe fn qualifier_string(size: u32, qualifier: *const c_void) -> Result<Option<String>, i32> {
    if size as usize != size_of::<*const c_void>() || qualifier.is_null() {
        return Err(BAD_SIZE);
    }
    let string = unsafe { qualifier.cast::<*const c_void>().read_unaligned() };
    if string.is_null() || unsafe { CFGetTypeID(string) != CFStringGetTypeID() } {
        return Err(ILLEGAL);
    }
    let mut bytes = [0_u8; 128];
    if unsafe { CFStringGetCString(string, bytes.as_mut_ptr().cast(), 128, UTF8) } == 0 {
        return Err(ILLEGAL);
    }
    let length = bytes.iter().position(|byte| *byte == 0).ok_or(ILLEGAL)?;
    std::str::from_utf8(&bytes[..length])
        .map(|value| Some(value.into()))
        .map_err(|_| ILLEGAL)
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_property_settable(
    registry: *mut Registry,
    object: u32,
    address: *const Address,
    output: *mut bool,
) -> i32 {
    let (Some(registry), Some(address), Some(output)) = (
        unsafe { registry.as_ref() },
        unsafe { address.as_ref() },
        unsafe { output.as_mut() },
    ) else {
        return ILLEGAL;
    };
    match settings::settable(registry, object, address) {
        Ok(value) => {
            *output = value;
            0
        }
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_property_set(
    registry: *mut Registry,
    object: u32,
    address: *const Address,
    size: u32,
    input: *const c_void,
) -> i32 {
    let (Some(registry), Some(address)) =
        (unsafe { registry.as_ref() }, unsafe { address.as_ref() })
    else {
        return ILLEGAL;
    };
    if input.is_null() {
        return ILLEGAL;
    }
    // SAFETY: the host supplies `size` readable bytes at the non-null input.
    unsafe { settings::set(registry, object, address, size, input) }
}
