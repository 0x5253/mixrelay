//! Per-registration unused-direction admission; device-wide stream settings are untouched.

use super::native::{self, Address, Properties, System};
use std::{
    ffi::c_void,
    ptr::{self, NonNull},
};

#[cfg(test)]
mod tests;

// Matches the aggregate stream bound used by native admission and listeners.
const MAX_STREAMS: usize = 2048;
const INVALID: i32 = -6;

#[repr(C)]
pub(super) struct Usage {
    pub(super) procedure: *mut c_void,
    pub(super) count: u32,
    pub(super) enabled: [u32; MAX_STREAMS],
}

pub(super) trait Backend: Properties {
    fn write_usage(&self, device: u32, scope: u32, usage: &Usage, bytes: u32) -> i32;
    fn read_usage(&self, device: u32, scope: u32, usage: &mut Usage, bytes: &mut u32) -> i32;
}

impl Backend for System {
    fn write_usage(&self, device: u32, scope: u32, usage: &Usage, bytes: u32) -> i32 {
        let address = Address::new(u32::from_be_bytes(*b"suse"), scope);
        // SAFETY: the validated extent contains the IOProc, stream count and all flags.
        unsafe {
            native::property_write(
                device,
                &raw const address,
                0,
                ptr::null(),
                bytes,
                ptr::from_ref(usage).cast(),
            )
        }
    }

    fn read_usage(&self, device: u32, scope: u32, usage: &mut Usage, bytes: &mut u32) -> i32 {
        let address = Address::new(u32::from_be_bytes(*b"suse"), scope);
        // SAFETY: initialized IOProc identity selects this registration. Writable storage
        // covers the bounded requested extent; returned extent/count are checked by caller.
        unsafe {
            native::property_read(
                device,
                &raw const address,
                0,
                ptr::null(),
                bytes,
                ptr::from_mut(usage).cast(),
            )
        }
    }
}

pub(super) fn disable_unused(
    backend: &impl Backend,
    device: u32,
    procedure: NonNull<c_void>,
    input: bool,
    limit: u32,
) -> i32 {
    let scope = u32::from_be_bytes(if input { *b"outp" } else { *b"inpt" });
    let Some(streams) = backend.streams(device, scope, limit) else {
        return INVALID;
    };
    let count = streams.count;
    if count > MAX_STREAMS || count > limit as usize || streams.values[..count].contains(&0) {
        return INVALID;
    }
    if count == 0 {
        return 0;
    }
    let Ok(native_count) = u32::try_from(count) else {
        return INVALID;
    };
    let Ok(bytes) = u32::try_from(std::mem::offset_of!(Usage, enabled) + count * size_of::<u32>())
    else {
        return INVALID;
    };
    let mut usage = Usage {
        procedure: procedure.as_ptr(),
        count: native_count,
        enabled: [0; MAX_STREAMS],
    };
    // Flexible C array extent excludes Rust/C trailing alignment padding.
    let status = backend.write_usage(device, scope, &usage, bytes);
    if status != 0 {
        return status;
    }
    // A failed or ignored Set must not accidentally pass because our requested flags were zero.
    usage.enabled.fill(1);
    let mut returned = bytes;
    let status = backend.read_usage(device, scope, &mut usage, &mut returned);
    if status != 0 {
        return status;
    }
    if returned != bytes
        || usage.procedure != procedure.as_ptr()
        || usage.count as usize != count
        || usage.enabled[..count].iter().any(|&on| on != 0)
    {
        return INVALID;
    }
    // Stream identity/order may change during either synchronous native call.
    if !backend
        .streams(device, scope, limit)
        .is_some_and(|current| {
            current.count == count && current.values[..count] == streams.values[..count]
        })
    {
        return INVALID;
    }
    0
}
