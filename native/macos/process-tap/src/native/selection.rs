//! Exact process identities and owned output-stream leases for native tap selection.

use super::properties::{cfstring_property_matches, object_ids, property, string_property};
use super::{
    Address, AudioObjectGetPropertyData, CFRelease, CFStringCreateWithCString, Native,
    OBJECT_ID_EXTENT, POINTER_EXTENT, TapError, mr_tap_description_supports_bundle_selection,
    mr_tap_description_supports_device_stream_selection,
};
use std::{
    ffi::{CStr, CString, c_void},
    ptr,
};

/// A process object alone is not a stable application identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProcessIdentity {
    pub object: u32,
    pub pid: i32,
}

/// One output device and its sole output stream at the time of tap admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceLease {
    pub object: u32,
    pub stream: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Selection {
    pub bundle: CString,
    pub exclusions: Option<Vec<CString>>,
    pub fallback: bool,
    pub selected: Vec<ProcessIdentity>,
    pub target_device_uid: Option<CString>,
    pub target_device: Option<DeviceLease>,
    pub mute: bool,
}

impl Selection {
    pub(crate) fn new(config: &crate::TapConfig, native: &impl Native) -> Result<Self, TapError> {
        // SAFETY: the public create boundary validated and retains this C string.
        let bundle = if config.system_audio {
            c"".to_owned()
        } else {
            // SAFETY: create validated this borrowed NUL-terminated string.
            unsafe { CStr::from_ptr(config.bundle_id) }.to_owned()
        };
        let exclusions = if config.system_audio {
            let mut ids = Vec::with_capacity(config.excluded_bundle_count);
            for index in 0..config.excluded_bundle_count {
                // SAFETY: create validated the array and each retained C string.
                ids.push(
                    unsafe { CStr::from_ptr(*config.excluded_bundle_ids.add(index)) }.to_owned(),
                );
            }
            ids.sort_unstable();
            ids.dedup();
            Some(ids)
        } else {
            None
        };
        let target_device_uid = if config.target_device_uid.is_null() {
            None
        } else {
            // SAFETY: create validated the NUL-terminated owned UID.
            Some(unsafe { CStr::from_ptr(config.target_device_uid) }.to_owned())
        };
        let target_device = target_device_uid
            .as_deref()
            .map(|uid| native.device(uid))
            .transpose()?;
        // A mutable bundle selector can acquire other output routes. Muted taps
        // must bind exact process objects and revalidate their device lease.
        let fallback = config.system_audio
            || target_device.is_some()
            || !unsafe { mr_tap_description_supports_bundle_selection() };
        if target_device.is_some()
            && !unsafe { mr_tap_description_supports_device_stream_selection() }
        {
            return Err(TapError::code(2));
        }
        Ok(Self {
            bundle,
            exclusions,
            fallback,
            selected: Vec::new(),
            target_device_uid,
            target_device,
            mute: config.mute_while_captured,
        })
    }

    pub(crate) fn processes(&self, native: &impl Native) -> Result<Vec<ProcessIdentity>, TapError> {
        if let Some(exclusions) = &self.exclusions {
            let mut result = Vec::new();
            for bundle in exclusions {
                result.extend(native.processes(bundle, None)?);
            }
            result.sort_unstable();
            result.dedup();
            Ok(result)
        } else {
            native.processes(&self.bundle, self.target_device.map(|lease| lease.object))
        }
    }
}
pub(crate) fn owned_endpoint_uid(uid: &CStr) -> bool {
    let bytes = uid.to_bytes();
    let Some(suffix) = bytes.strip_prefix(b"io.mixrelay.audio.endpoint.") else {
        return false;
    };
    suffix.len() == 32
        && suffix
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && suffix.iter().any(|byte| *byte != b'0')
}

pub(super) fn device(uid: &CStr) -> Result<DeviceLease, TapError> {
    // SAFETY: CF copies the owned UTF-8 UID and returns one local reference.
    let expected = unsafe { CFStringCreateWithCString(ptr::null(), uid.as_ptr(), 0x0800_0100) };
    if expected.is_null() {
        return Err(TapError::code(6));
    }
    let mut object = 0_u32;
    let mut size = OBJECT_ID_EXTENT;
    // SAFETY: the qualifier contains one live CFString pointer and result owns one UInt32.
    let status = unsafe {
        AudioObjectGetPropertyData(
            1,
            &Address::global(*b"uidd"),
            POINTER_EXTENT,
            ptr::from_ref(&expected).cast(),
            &raw mut size,
            ptr::from_mut(&mut object).cast(),
        )
    };
    let stream = (status == 0 && size as usize == size_of::<u32>() && object != 0)
        .then(|| object_ids(object, Address::output(*b"stm#")))
        .and_then(Result::ok)
        .filter(|streams| streams.len() == 1 && streams[0] != 0)
        .map(|streams| streams[0]);
    let valid = status == 0
        && size as usize == size_of::<u32>()
        && object != 0
        && cfstring_property_matches(object, Address::global(*b"uid "), expected)
        && stream.is_some();
    // SAFETY: translation and validation are complete; no native reference escapes.
    unsafe { CFRelease(expected) };
    stream
        .filter(|_| valid)
        .map(|stream| DeviceLease { object, stream })
        .ok_or(TapError::code(9))
}

fn process_uses_output_device(process: u32, target: u32) -> bool {
    object_ids(process, Address::output(*b"pdv#")).is_ok_and(|devices| devices.contains(&target))
}

pub(super) fn processes(
    bundle: &CStr,
    target_device: Option<u32>,
) -> Result<Vec<ProcessIdentity>, TapError> {
    let ids = object_ids(1, Address::global(*b"prs#"))?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // SAFETY: owned UTF-8 C string remains live; CF copies it synchronously.
    let expected = unsafe { CFStringCreateWithCString(ptr::null(), bundle.as_ptr(), 0x0800_0100) };
    if expected.is_null() {
        return Err(TapError::code(6));
    }
    let mut selected = Vec::new();
    for object in ids {
        // Recheck both properties around the same object ID so a disappearing
        // process cannot be paired with the PID of an unrelated replacement.
        if process_matches(object, expected)
            && let Ok(pid) = property::<i32>(object, *b"ppid")
            && pid > 0
            && target_device.is_none_or(|device| process_uses_output_device(object, device))
            && process_matches(object, expected)
            && property::<i32>(object, *b"ppid").is_ok_and(|again| again == pid)
        {
            selected.push(ProcessIdentity { object, pid });
        }
    }
    // SAFETY: owned expected CFString no longer used after filtering.
    unsafe { CFRelease(expected) };
    selected.sort_unstable();
    selected.dedup();
    Ok(selected)
}

/// Every running process's use of audio devices, from `CoreAudio` process objects: which
/// device UIDs it is playing into (running output) and recording from (running input).
pub(crate) fn device_uses() -> Result<Vec<crate::DeviceUse>, TapError> {
    let mut uses = Vec::new();
    for process in object_ids(1, Address::global(*b"prs#"))? {
        let (Ok(process_id), Some(bundle_id)) = (
            property::<i32>(process, *b"ppid"),
            string_property(process, Address::global(*b"pbid")),
        ) else {
            continue;
        };
        let playing = property::<u32>(process, *b"piro").is_ok_and(|running| running != 0);
        let recording = property::<u32>(process, *b"piri").is_ok_and(|running| running != 0);
        let uids = |address: Address, active: bool| -> Vec<String> {
            if !active {
                return Vec::new();
            }
            object_ids(process, address)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|device| string_property(device, Address::global(*b"uid ")))
                .collect()
        };
        crate::push_device_uses(
            &mut uses,
            process_id,
            &bundle_id,
            &uids(Address::output(*b"pdv#"), playing),
            &uids(Address::input(*b"pdv#"), recording),
        );
    }
    Ok(uses)
}

fn process_matches(process: u32, expected: *const c_void) -> bool {
    cfstring_property_matches(process, Address::global(*b"pbid"), expected)
}
