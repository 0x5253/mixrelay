//! Typed `CoreAudio` property reads and exact ownership of returned CF references.

use super::{
    Address, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, CFEqual, CFRelease,
    CFStringGetCString, POINTER_EXTENT, TapError,
};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
};

pub(super) fn property<T: Default>(object: u32, selector: [u8; 4]) -> Result<T, TapError> {
    property_at(object, Address::global(selector))
}

pub(super) fn property_at<T: Default>(object: u32, address: Address) -> Result<T, TapError> {
    let mut value = T::default();
    let mut size = u32::try_from(size_of::<T>()).map_err(|_| TapError::code(5))?;
    // SAFETY: typed output owns size bytes; address/size live through synchronous query.
    TapError::status(unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            ptr::from_mut(&mut value).cast(),
        )
    })?;
    if size as usize != size_of::<T>() {
        return Err(TapError::code(5));
    }
    Ok(value)
}

pub(super) fn object_ids(object: u32, address: Address) -> Result<Vec<u32>, TapError> {
    let mut size = 0;
    // SAFETY: initialized property address and writable byte count remain live.
    TapError::status(unsafe {
        AudioObjectGetPropertyDataSize(object, &raw const address, 0, ptr::null(), &raw mut size)
    })?;
    if !size.is_multiple_of(4) || size > 4096 * 4 {
        return Err(TapError::code(5));
    }
    let mut ids = Vec::new();
    ids.try_reserve_exact(size as usize / 4)
        .map_err(|_| TapError::code(6))?;
    ids.resize(size as usize / 4, 0_u32);
    if ids.is_empty() {
        return Ok(ids);
    }
    // SAFETY: the vector owns at least the queried byte extent; a growth race
    // is rejected using the returned size before any element is inspected.
    TapError::status(unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            ids.as_mut_ptr().cast(),
        )
    })?;
    if !size.is_multiple_of(4) || size as usize > ids.len() * 4 {
        return Err(TapError::code(9));
    }
    ids.truncate(size as usize / 4);
    Ok(ids)
}

pub(super) fn cfstring_property_matches(
    object: u32,
    address: Address,
    expected: *const c_void,
) -> bool {
    let mut actual: *const c_void = ptr::null();
    let mut size = POINTER_EXTENT;
    // SAFETY: exclusive pointer output starts null. Every non-null +1 result is
    // adopted before interpreting either status or returned property extent.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            ptr::from_mut(&mut actual).cast(),
        )
    };
    finish_property(
        (status, size, actual),
        |actual| {
            // SAFETY: only successful correctly sized CFString output reaches comparison.
            unsafe { CFEqual(expected, actual) != 0 }
        },
        |actual| {
            // SAFETY: property output transfers a +1 reference even on partial failure.
            unsafe { CFRelease(actual) };
        },
    )
}

pub(crate) fn finish_property(
    result: (i32, u32, *const c_void),
    compare: impl FnOnce(*const c_void) -> bool,
    release: impl FnMut(*const c_void),
) -> bool {
    struct Owned<F: FnMut(*const c_void)> {
        value: *const c_void,
        release: F,
    }
    impl<F: FnMut(*const c_void)> Drop for Owned<F> {
        fn drop(&mut self) {
            (self.release)(self.value);
        }
    }
    let (status, size, value) = result;
    if value.is_null() {
        return false;
    }
    let owner = Owned { value, release };
    status == 0 && size as usize == size_of::<*const c_void>() && compare(owner.value)
}
/// A `CFString` property as UTF-8, or `None` when absent, unreadable or longer than 1 KiB.
pub(super) fn string_property(object: u32, address: Address) -> Option<String> {
    let mut value: *const c_void = ptr::null();
    let mut size = POINTER_EXTENT;
    // SAFETY: exclusive pointer output starts null; a non-null result is a +1 reference
    // released below on every path.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const address,
            0,
            ptr::null(),
            &raw mut size,
            ptr::from_mut(&mut value).cast(),
        )
    };
    let mut text = None;
    finish_property(
        (status, size, value),
        |value| {
            let mut buffer = [0 as c_char; 1024];
            // SAFETY: the buffer is writable for its full length and CF null-terminates it.
            let copied =
                unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), 1024, 0x0800_0100) != 0 };
            if copied {
                // SAFETY: CFStringGetCString succeeded, so the buffer holds a C string.
                text = unsafe { CStr::from_ptr(buffer.as_ptr()) }
                    .to_str()
                    .ok()
                    .map(str::to_owned);
            }
            copied
        },
        |value| {
            // SAFETY: property output transfers a +1 reference even on partial failure.
            unsafe { CFRelease(value) };
        },
    );
    text
}
