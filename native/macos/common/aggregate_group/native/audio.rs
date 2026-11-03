use std::ffi::c_void;

use super::cf::Object;
use crate::aggregate_group::MemberInfo;
use crate::audio_buffer::AudioBuffer;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioHardwareCreateAggregateDevice(description: *const c_void, device: *mut u32) -> i32;
    fn AudioHardwareDestroyAggregateDevice(device: u32) -> i32;
}

const GLOBAL: u32 = u32::from_be_bytes(*b"glob");

fn property<T: Default>(device: u32, selector: [u8; 4]) -> Option<T> {
    let mut value = T::default();
    let mut size = u32::try_from(size_of::<T>()).ok()?;
    let address = Address {
        selector: u32::from_be_bytes(selector),
        scope: GLOBAL,
        element: 0,
    };
    // SAFETY: each local call uses the SDK's matching scalar/pointer property type;
    // the writable buffer has exactly size_of::<T>() bytes and is not retained.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
            (&raw mut value).cast(),
        )
    };
    (status == 0 && size as usize == size_of::<T>()).then_some(value)
}

fn object(device: u32, selector: [u8; 4]) -> Option<Object> {
    let mut pointer: *const c_void = std::ptr::null();
    let mut size = u32::try_from(size_of::<*const c_void>()).ok()?;
    let address = Address {
        selector: u32::from_be_bytes(selector),
        scope: GLOBAL,
        element: 0,
    };
    // SAFETY: these selectors return one owned CF reference. Even on an invalid
    // status/size, any returned reference is adopted before rejecting the response.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
            (&raw mut pointer).cast(),
        )
    };
    // SAFETY: these CF-valued CoreAudio properties transfer a +1 reference.
    let object = unsafe { Object::owned(pointer) };
    accept_object(object, status, size)
}

pub(super) fn accept_object(object: Option<Object>, status: i32, size: u32) -> Option<Object> {
    if status != 0 || size as usize != size_of::<*const c_void>() {
        return None;
    }
    object
}

fn resolve(uid: &Object) -> Option<u32> {
    let mut device = 0;
    let mut size = 4;
    let pointer = uid.as_ptr();
    let address = Address {
        selector: u32::from_be_bytes(*b"uidd"),
        scope: GLOBAL,
        element: 0,
    };
    // SAFETY: qualifier contains one live CFStringRef; output is one AudioDeviceID.
    let status = unsafe {
        AudioObjectGetPropertyData(
            1,
            &raw const address,
            8,
            (&raw const pointer).cast(),
            &raw mut size,
            (&raw mut device).cast(),
        )
    };
    (status == 0 && size == 4 && device != 0).then_some(device)
}

/// The `CoreAudio` calls a private aggregate makes. Production uses [`System`]; hermetic
/// tests supply a simulated HAL beneath the same planning, readback and retirement logic.
pub(super) trait Hal {
    /// The device a persistent UID currently names.
    fn device(&self, uid: &Object) -> Option<u32>;
    /// One global u32 property, such as transport type or liveness.
    fn scalar(&self, device: u32, selector: [u8; 4]) -> Option<u32>;
    /// The nominal sample rate.
    fn rate(&self, device: u32) -> Option<f64>;
    /// Total channels of one direction's stream configuration.
    fn channels(&self, device: u32, input: bool) -> Option<u32>;
    /// One CF-valued global property, returned with its owned reference.
    fn object(&self, device: u32, selector: [u8; 4]) -> Option<Object>;
    /// Fills `output` with the active subdevices; false unless exactly that many are active.
    fn active(&self, device: u32, output: &mut [u32]) -> bool;
    /// Publishes an aggregate for `description`; a written device transfers even on error.
    fn create(&self, description: &Object, device: &mut u32) -> i32;
    fn destroy(&self, device: u32) -> i32;
}

/// The process's `CoreAudio` HAL.
pub(super) struct System;

impl Hal for System {
    fn device(&self, uid: &Object) -> Option<u32> {
        resolve(uid)
    }
    fn scalar(&self, device: u32, selector: [u8; 4]) -> Option<u32> {
        property(device, selector)
    }
    fn rate(&self, device: u32) -> Option<f64> {
        property(device, *b"nsrt")
    }
    fn channels(&self, device: u32, input: bool) -> Option<u32> {
        channels(device, input)
    }
    fn object(&self, device: u32, selector: [u8; 4]) -> Option<Object> {
        object(device, selector)
    }
    fn active(&self, device: u32, output: &mut [u32]) -> bool {
        active(device, output)
    }
    fn create(&self, description: &Object, device: &mut u32) -> i32 {
        create(description, device)
    }
    fn destroy(&self, device: u32) -> i32 {
        destroy(device)
    }
}

/// Current facts of one physical member; the transport type only when requested.
pub(super) fn member(hal: &impl Hal, uid: &Object, transport: bool) -> Option<MemberInfo> {
    let device = hal.device(uid)?;
    Some(MemberInfo {
        device,
        transport: if transport {
            hal.scalar(device, *b"tran")?
        } else {
            0
        },
        rate: hal.rate(device)?,
        alive: hal.scalar(device, *b"livn")?,
        input: hal.channels(device, true)?,
        output: hal.channels(device, false)?,
    })
}

fn channels(device: u32, input: bool) -> Option<u32> {
    let address = Address {
        selector: u32::from_be_bytes(*b"slay"),
        scope: u32::from_be_bytes(if input { *b"inpt" } else { *b"outp" }),
        element: 0,
    };
    let mut size = 0;
    // SAFETY: the query writes one size; it does not retain the local address.
    if unsafe {
        AudioObjectGetPropertyDataSize(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
        )
    } != 0
        || !(8..=65536).contains(&size)
    {
        return None;
    }
    // u64 storage supplies AudioBufferList alignment, with a bounded fallible allocation.
    let words = (size as usize).div_ceil(8);
    let mut storage = Vec::<u64>::new();
    storage.try_reserve_exact(words).ok()?;
    storage.resize(words, 0);
    let allocated = size;
    // SAFETY: storage is aligned and holds at least the queried byte count; no native pointer escapes.
    if unsafe {
        AudioObjectGetPropertyData(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
            storage.as_mut_ptr().cast(),
        )
    } != 0
        || size > allocated
    {
        return None;
    }
    parse_channels(&storage, size as usize)
}

pub(super) fn parse_channels(storage: &[u64], bytes: usize) -> Option<u32> {
    if bytes < 8 || bytes > storage.len().checked_mul(8)? {
        return None;
    }
    let first_u32 = |word: u64| {
        let bytes = word.to_ne_bytes();
        u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    };
    let count = first_u32(*storage.first()?) as usize;
    if count > (bytes - 8) / size_of::<AudioBuffer>() {
        return None;
    }
    // SDK AudioBufferList has one header word followed by two words per buffer.
    // Only each buffer's first u32 is needed; never reinterpret its PCM pointer.
    storage[1..]
        .chunks_exact(2)
        .take(count)
        .try_fold(0_u32, |total, buffer| {
            total
                .checked_add(first_u32(buffer[0]))
                .filter(|sum| *sum <= 2048)
        })
}

fn active(device: u32, output: &mut [u32]) -> bool {
    let Ok(mut size) = u32::try_from(size_of_val(output)) else {
        return false;
    };
    let expected = size;
    let address = Address {
        selector: u32::from_be_bytes(*b"agrp"),
        scope: GLOBAL,
        element: 0,
    };
    // SAFETY: output is a bounded initialized u32 array, matching the active AudioDeviceID property.
    unsafe {
        AudioObjectGetPropertyData(
            device,
            &raw const address,
            0,
            std::ptr::null(),
            &raw mut size,
            output.as_mut_ptr().cast(),
        ) == 0
            && size == expected
    }
}

fn create(description: &Object, device: &mut u32) -> i32 {
    // SAFETY: description is a live CFDictionary with the documented aggregate keys;
    // the group retains device even if native creation reports an error.
    unsafe { AudioHardwareCreateAggregateDevice(description.as_ptr(), device) }
}

fn destroy(device: u32) -> i32 {
    // SAFETY: only the exclusive retirement owner calls this after all native callbacks drain.
    unsafe { AudioHardwareDestroyAggregateDevice(device) }
}
