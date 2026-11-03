//! `CFData` payloads carried by `AudioServerPlugIn` custom property lists.
use std::ffi::c_void;

pub(crate) const DATA_TYPE: u32 = u32::from_be_bytes(*b"plst");

#[derive(Clone, Copy, Debug)]
pub(crate) enum DataError {
    Size,
    Type,
    Allocation,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, count: isize) -> *const c_void;
    fn CFDataGetTypeID() -> usize;
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFDataGetLength(value: *const c_void) -> isize;
    fn CFDataGetBytePtr(value: *const c_void) -> *const u8;
}

/// # Safety
/// Non-null output holds capacity writable bytes. The caller owns the returned
/// +1 `CFData` reference and releases it after marshalling.
pub(crate) unsafe fn write(
    bytes: &[u8],
    capacity: usize,
    output: *mut c_void,
) -> Result<usize, DataError> {
    if output.is_null() || capacity < size_of::<*const c_void>() {
        return Err(DataError::Size);
    }
    let count = isize::try_from(bytes.len()).map_err(|_| DataError::Size)?;
    // SAFETY: bytes contains count readable bytes; a null allocator is permitted.
    let data = unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), count) };
    if data.is_null() {
        return Err(DataError::Allocation);
    }
    // SAFETY: caller owns pointer-sized output, which need not be aligned.
    unsafe { output.cast::<*const c_void>().write_unaligned(data) };
    Ok(size_of::<*const c_void>())
}

/// # Safety
/// Input contains size readable bytes and, when pointer-sized, one live `CFTypeRef`
/// supplied by the host's property-list marshaller. Its contents stay immutable.
pub(crate) unsafe fn read_flag(size: u32, input: *const c_void) -> Result<bool, DataError> {
    if input.is_null() || size as usize != size_of::<*const c_void>() {
        return Err(DataError::Size);
    }
    // SAFETY: caller provides a readable CF object reference for this call.
    let data = unsafe { input.cast::<*const c_void>().read_unaligned() };
    if data.is_null() || unsafe { CFGetTypeID(data) != CFDataGetTypeID() } {
        return Err(DataError::Type);
    }
    // SAFETY: the type check established a live immutable CFData.
    if unsafe { CFDataGetLength(data) } != 4 {
        return Err(DataError::Size);
    }
    // SAFETY: this CFData has four readable bytes; alignment is not required.
    let value = unsafe { CFDataGetBytePtr(data).cast::<u32>().read_unaligned() };
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DataError::Type),
    }
}
