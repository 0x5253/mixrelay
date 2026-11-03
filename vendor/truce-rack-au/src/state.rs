//! Property-list serialization and native class-info state transfer.

use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnitGetProperty, AudioUnitSetProperty,
    kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global,
};
use objc2_core_foundation::{
    CFData, CFPropertyList, CFPropertyListCreateData, CFPropertyListCreateWithData,
    CFPropertyListFormat, CFRetained,
};
use std::ptr;
use truce_rack_core::error::{Error, Result};

/// Capture the native class-info property as a binary property list.
///
/// # Safety
/// The live unit is exclusively controlled on its creator thread outside rendering.
pub(super) unsafe fn capture(unit: AudioComponentInstance) -> Result<Vec<u8>> {
    let mut class_info: *const CFPropertyList = ptr::null();
    let mut size = u32::try_from(std::mem::size_of::<*const CFPropertyList>()).unwrap_or(0);
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_ClassInfo,
            kAudioUnitScope_Global,
            0,
            ptr::NonNull::new_unchecked((&raw mut class_info).cast()),
            ptr::NonNull::new_unchecked(&raw mut size),
        )
    };
    if status != 0 || class_info.is_null() {
        return Err(Error::Other(format!(
            "kAudioUnitProperty_ClassInfo failed: OSStatus {status}"
        )));
    }
    let plist = unsafe {
        CFRetained::<CFPropertyList>::from_raw(ptr::NonNull::new_unchecked(class_info.cast_mut()))
    };
    let mut error: *mut objc2_core_foundation::CFError = ptr::null_mut();
    let data: Option<CFRetained<CFData>> = unsafe {
        CFPropertyListCreateData(
            None,
            Some(&plist),
            CFPropertyListFormat::BinaryFormat_v1_0,
            0,
            &raw mut error,
        )
    };
    // SAFETY: each non-null error returned by the Create API is a caller-owned reference.
    let _error = ptr::NonNull::new(error).map(|raw| unsafe { CFRetained::from_raw(raw) });
    let data = data.ok_or_else(|| Error::Other("CFPropertyListCreateData returned null".into()))?;
    let len = data.length();
    if len < 0 {
        return Err(Error::Other("CFData length negative".into()));
    }
    #[allow(clippy::cast_sign_loss)]
    let len_usize = len as usize;
    let mut out = vec![0u8; len_usize];
    unsafe {
        data.bytes(
            objc2_core_foundation::CFRange {
                location: 0,
                length: len,
            },
            out.as_mut_ptr(),
        );
    }
    Ok(out)
}

pub(super) fn decode(bytes: &[u8]) -> Result<CFRetained<CFPropertyList>> {
    if bytes.is_empty() {
        return Err(Error::Other("empty AU state".into()));
    }
    #[allow(clippy::cast_possible_wrap)]
    let cf_data: CFRetained<CFData> =
        unsafe { CFData::new(None, bytes.as_ptr(), bytes.len() as isize) }
            .ok_or_else(|| Error::Other("CFData::new returned null".into()))?;
    let mut error: *mut objc2_core_foundation::CFError = ptr::null_mut();
    let mut format = CFPropertyListFormat::BinaryFormat_v1_0;
    let plist: Option<CFRetained<CFPropertyList>> = unsafe {
        CFPropertyListCreateWithData(None, Some(&cf_data), 0, &raw mut format, &raw mut error)
    };
    // SAFETY: each non-null error returned by the Create API is a caller-owned reference.
    let _error = ptr::NonNull::new(error).map(|raw| unsafe { CFRetained::from_raw(raw) });
    let plist =
        plist.ok_or_else(|| Error::Other("CFPropertyListCreateWithData returned null".into()))?;
    Ok(plist)
}

/// Apply the decoded property list to one native instance.
///
/// # Safety
/// The live unit is exclusively controlled on its creator thread outside rendering. A native
/// rejection may leave partial state; the host must retire that prepared instance on failure.
pub(super) unsafe fn restore(unit: AudioComponentInstance, plist: &CFPropertyList) -> Result<()> {
    let plist_ptr: *const CFPropertyList = ptr::from_ref(plist);
    // SAFETY: the unit and decoded property list remain live throughout the serialized call.
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            kAudioUnitProperty_ClassInfo,
            kAudioUnitScope_Global,
            0,
            (&raw const plist_ptr).cast(),
            u32::try_from(std::mem::size_of::<*const CFPropertyList>()).unwrap_or(0),
        )
    };
    if status != 0 {
        return Err(Error::Other(format!(
            "AudioUnitSetProperty(ClassInfo) failed: OSStatus {status}"
        )));
    }
    Ok(())
}
