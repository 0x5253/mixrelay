//! C ABI over [`SourceRegistry`] for the engine service and native fixtures.
use super::{SourceClock, SourceRegistry, identity::token};
use crate::source_control::SourceRequest;
use std::{ffi::c_char, os::fd::IntoRawFd};

#[unsafe(no_mangle)]
pub extern "C" fn mr_engine_source_registry_create() -> *mut SourceRegistry {
    Box::into_raw(Box::new(SourceRegistry::new(
        Box::new(mixrelay_time::StdClock::new()),
        mixrelay_realtime::Timebase::current(),
    )))
}

/// # Safety
/// Registry was created by this module; all callers/readers have stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_destroy(registry: *mut SourceRegistry) {
    if !registry.is_null() {
        drop(unsafe { Box::from_raw(registry) });
    }
}

/// # Safety
/// Registry/request and all output pointers are live. A successful descriptor
/// is owned by the caller and must be closed after the attachment is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_attach_clocked(
    registry: *mut SourceRegistry,
    request: *const SourceRequest,
    descriptor: *mut i32,
    bytes: *mut usize,
    lease: *mut u64,
    epoch: *mut u64,
) -> bool {
    let (Some(registry), Some(request)) =
        (unsafe { registry.as_ref() }, unsafe { request.as_ref() })
    else {
        return false;
    };
    if descriptor.is_null() || bytes.is_null() || lease.is_null() || epoch.is_null() {
        return false;
    }
    let Ok(attachment) = registry.attach(request) else {
        return false;
    };
    unsafe {
        descriptor.write(attachment.descriptor.into_raw_fd());
        bytes.write(attachment.mapped_size);
        lease.write(attachment.producer_lease);
        epoch.write(attachment.source_epoch);
    }
    true
}

/// # Safety
/// Source ID is a NUL-terminated string of at most 127 bytes, or a readable
/// 128-byte unterminated array (which is rejected).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_id_token(id: *const c_char) -> u64 {
    if id.is_null() {
        return 0;
    }
    let mut bytes = [0_u8; 128];
    for index in 0..128 {
        let byte = unsafe { id.add(index).read() }.to_ne_bytes()[0];
        if byte == 0 {
            return if index == 0 {
                0
            } else {
                token(&bytes[..index])
            };
        }
        bytes[index] = byte;
    }
    0
}

/// # Safety
/// Registry is live and output is writable and disjoint from shared memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_clock(
    registry: *mut SourceRegistry,
    token: u64,
    output: *mut SourceClock,
) -> bool {
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    let Some(clock) = registry.clock(token) else {
        return false;
    };
    unsafe {
        output.write(clock);
    }
    true
}

/// # Safety
/// Registry remains live throughout this control call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_set_clock_master(
    registry: *mut SourceRegistry,
    token: u64,
    master: bool,
) -> bool {
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        return false;
    };
    registry.set_clock_master(token, master)
}

/// # Safety
/// Registry is live; samples has frames*channels writable floats and does not
/// alias the transport or another output buffer during this nonblocking call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_read_clocked(
    registry: *mut SourceRegistry,
    token: u64,
    samples: *mut f32,
    frames: u32,
    channels: u32,
    rate: u32,
) -> u32 {
    if samples.is_null() || frames > 1_048_576 || channels == 0 || channels > 64 {
        return 0;
    }
    let output =
        unsafe { std::slice::from_raw_parts_mut(samples, frames as usize * channels as usize) };
    let Some(registry) = (unsafe { registry.as_ref() }) else {
        output.fill(0.0);
        return 0;
    };
    registry.read_audio(token, output, frames, rate)
}

/// Drains normalized source samples once, independently of PCM availability.
/// # Safety
/// Registry is null or retained through this call; the graph owns this source consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_engine_source_registry_take_source_faults(
    registry: *const SourceRegistry,
    token: u64,
) -> u64 {
    // SAFETY: the read guard pins the source; only its atomic counter is consumed.
    unsafe { registry.as_ref() }.map_or(0, |registry| registry.take_source_faults(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_token_matches_the_registry_token_and_rejects_empty_or_unterminated_ids() {
        // SAFETY: each pointer is null or a NUL-terminated/128-byte readable string.
        unsafe {
            assert_eq!(mr_engine_source_id_token(c"app".as_ptr()), token(b"app"));
            assert_eq!(mr_engine_source_id_token(std::ptr::null()), 0);
            assert_eq!(mr_engine_source_id_token(c"".as_ptr()), 0);
            let unterminated = [c_char::from_ne_bytes([b'a']); 128];
            assert_eq!(mr_engine_source_id_token(unterminated.as_ptr()), 0);
            let longest = [[b'a'; 127].as_slice(), &[0]].concat();
            assert_eq!(
                mr_engine_source_id_token(longest.as_ptr().cast()),
                token(&[b'a'; 127])
            );
        }
    }
}
