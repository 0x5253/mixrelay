//! Exact own-device Core Audio property access; no default selection or graph clock policy.
use crate::endpoint_registry::EndpointId;
use mixrelay_audio_io::{
    ChannelLayout, NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat,
    NativeStreamFormats, StreamSpec, VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_driver_control::{DriverDeviceConfig, DriverError};
use mixrelay_macos_transport::{
    control_inventory::MAX_ENTRIES,
    endpoint_identity::{endpoint_uid, format_endpoint_id, parse_endpoint_id},
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
};

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Range {
    minimum: f64,
    maximum: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Asbd {
    rate: f64,
    format: u32,
    flags: u32,
    packet_bytes: u32,
    packet_frames: u32,
    frame_bytes: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RangedFormat {
    format: Asbd,
    range: Range,
}
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        value: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFRelease(value: *const c_void);
    fn CFStringGetCString(
        value: *const c_void,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
}
fn address(selector: [u8; 4]) -> Address {
    Address {
        selector: u32::from_be_bytes(selector),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    }
}
fn read<T>(object: u32, property: &Address, value: &mut T) -> Option<usize> {
    let capacity = size_of_val(value);
    let mut size = u32::try_from(capacity).ok()?;
    // SAFETY: Core Audio receives the initialized aligned writable T extent for this
    // synchronous read. Returned byte count is bounded before any array is sliced.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            property,
            0,
            ptr::null(),
            &raw mut size,
            std::ptr::from_mut(value).cast(),
        )
    };
    (status == 0 && size as usize <= capacity).then_some(size as usize)
}

fn parse_native(format: Asbd) -> Option<NativeStreamFormat> {
    let sample_rate = mixrelay_audio_io::formats::SAMPLE_RATES
        .iter()
        .copied()
        .find(|rate| f64::from(*rate).to_bits() == format.rate.to_bits())?;
    let (sample_format, sample_bytes) = match (format.flags, format.bits) {
        (9, 32) => (NativeSampleFormat::Float32, 4),
        (12, 16) => (NativeSampleFormat::Signed16, 2),
        (12, 24) => (NativeSampleFormat::Signed24, 3),
        (12, 32) => (NativeSampleFormat::Signed32, 4),
        _ => return None,
    };
    if format.format != u32::from_be_bytes(*b"lpcm")
        || !(1..=64).contains(&format.channels)
        || format.packet_frames != 1
        || format.reserved != 0
        || format.frame_bytes != format.channels * sample_bytes
        || format.packet_bytes != format.frame_bytes
    {
        return None;
    }
    Some(NativeStreamFormat {
        sample_rate,
        channels: u16::try_from(format.channels).ok()?,
        sample_format,
    })
}

fn stream_formats(object: u32, scope: [u8; 4]) -> NativeStreamFormats {
    let mut property = address(*b"stm#");
    property.scope = u32::from_be_bytes(scope);
    let mut stream = 0;
    if read(object, &property, &mut stream) != Some(4) || stream == 0 {
        return NativeStreamFormats::default();
    }
    let mut current = Asbd::default();
    let current = (read(stream, &address(*b"pft "), &mut current) == Some(size_of::<Asbd>()))
        .then(|| parse_native(current))
        .flatten();
    // The owned HAL has six rates and four packed encodings per directional stream.
    let mut ranged = [RangedFormat::default(); 24];
    let supported = read(stream, &address(*b"pfta"), &mut ranged)
        .filter(|size| size.is_multiple_of(size_of::<RangedFormat>()))
        .map(|size| {
            ranged[..size / size_of::<RangedFormat>()]
                .iter()
                .filter_map(|value| {
                    if value.range.minimum.to_bits() == value.format.rate.to_bits()
                        && value.range.maximum.to_bits() == value.format.rate.to_bits()
                    {
                        parse_native(value.format)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    NativeStreamFormats { current, supported }
}

pub(super) fn native_formats(id: EndpointId) -> NativeDeviceFormats {
    let Some(object) = device(id) else {
        return NativeDeviceFormats::default();
    };
    NativeDeviceFormats {
        render: stream_formats(object, *b"outp"),
        capture: stream_formats(object, *b"inpt"),
    }
}
pub(super) fn device(id: EndpointId) -> Option<u32> {
    translate(*b"uidd", &endpoint_uid(id))
}

fn translate(selector: [u8; 4], value: &str) -> Option<u32> {
    let name = CString::new(value).ok()?;
    // SAFETY: name is a terminated owned UTF-8 buffer retained through creation;
    // the create rule returns a separately owned CFString, released below.
    let uid = unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100) };
    if uid.is_null() {
        return None;
    }
    let mut object = 0_u32;
    let mut size = u32::try_from(size_of_val(&object)).ok()?;
    let property = address(selector);
    // SAFETY: the qualifier is one live CFStringRef pointer. Core Audio writes only
    // the declared u32 output capacity; all locals remain aligned and disjoint.
    let result = unsafe {
        AudioObjectGetPropertyData(
            1,
            &raw const property,
            u32::try_from(size_of_val(&uid)).ok()?,
            (&raw const uid).cast(),
            &raw mut size,
            (&raw mut object).cast(),
        )
    };
    // SAFETY: release exactly our +1 create ownership after the synchronous lookup.
    unsafe { CFRelease(uid) };
    (result == 0 && size == 4 && object != 0).then_some(object)
}

fn string(object: u32, selector: [u8; 4]) -> Option<String> {
    let mut value: *const c_void = ptr::null();
    if read(object, &address(selector), &mut value) != Some(size_of::<*const c_void>())
        || value.is_null()
    {
        return None;
    }
    let mut bytes = [0_i8; 512];
    // SAFETY: Core Audio transfers a +1 CFString for UID/name properties. The
    // conversion writes only our initialized buffer, and release balances +1.
    let copied = unsafe { CFStringGetCString(value, bytes.as_mut_ptr(), 512, 0x0800_0100) };
    unsafe { CFRelease(value) };
    if copied == 0 {
        return None;
    }
    // SAFETY: successful CFStringGetCString includes a NUL in the buffer extent.
    unsafe { CStr::from_ptr(bytes.as_ptr()) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

fn endpoint_from_uid(uid: &str) -> Option<EndpointId> {
    let suffix = uid.strip_prefix("io.mixrelay.audio.endpoint.")?;
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    // Canonical ASCII hex permits these byte slices; reuse the shared UUID parser.
    parse_endpoint_id(&format!(
        "{}-{}-{}-{}-{}",
        &suffix[..8],
        &suffix[8..12],
        &suffix[12..16],
        &suffix[16..20],
        &suffix[20..]
    ))
}

fn observed_stream(object: u32, scope: [u8; 4]) -> Result<Option<NativeStreamFormat>, DriverError> {
    let mut property = address(*b"stm#");
    property.scope = u32::from_be_bytes(scope);
    let mut stream = 0_u32;
    match read(object, &property, &mut stream) {
        Some(0) => Ok(None),
        Some(4) if stream != 0 => {
            stream_formats(object, scope)
                .current
                .map(Some)
                .ok_or_else(|| {
                    DriverError::Backend(
                        "owned HAL stream has no exact native ASBD readback".into(),
                    )
                })
        }
        _ => Err(DriverError::Backend(
            "owned HAL directional stream enumeration failed".into(),
        )),
    }
}

fn observed_config(
    id: EndpointId,
    name: String,
    rate: u32,
    render: Option<NativeStreamFormat>,
    capture: Option<NativeStreamFormat>,
) -> Result<DriverDeviceConfig, DriverError> {
    if render.is_none() && capture.is_none()
        || [render, capture]
            .into_iter()
            .flatten()
            .any(|format| format.sample_rate != rate)
    {
        return Err(DriverError::Backend(
            "owned HAL native streams disagree with nominal rate or are absent".into(),
        ));
    }
    let stream = |format: NativeStreamFormat| StreamSpec {
        channels: format.channels,
        channel_layout: match format.channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            _ => ChannelLayout::Discrete,
        },
    };
    Ok(DriverDeviceConfig {
        spec: VirtualEndpointSpec {
            id: VirtualEndpointId::new(format_endpoint_id(id)),
            display_name: name,
            render: render.map(stream),
            capture: capture.map(stream),
            supported_sample_rates: mixrelay_audio_io::formats::SAMPLE_RATES.to_vec(),
            default_sample_rate: rate,
        },
        render_format: render,
        capture_format: capture,
    })
}

/// Reads only the exact installed HAL plugin's complete actual device inventory.
/// A failed/oversized list or unreadable native tuple is never treated as empty.
pub(super) fn observed_inventory() -> Result<Vec<DriverDeviceConfig>, DriverError> {
    // CFBundleIdentifier in native/macos/hal-driver/driver/Info.plist, not an
    // inferred Mach service label. SDK 'bidp' resolves only that plugin object.
    let plugin = translate(*b"bidp", "io.mixrelay.audio.driver").ok_or_else(|| {
        DriverError::Unavailable("installed HAL plugin is not discoverable".into())
    })?;
    let property = address(*b"dev#");
    let mut size = 0;
    // SAFETY: synchronous scalar size query for the own-plugin device list.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(plugin, &raw const property, 0, ptr::null(), &raw mut size)
    };
    let mut objects = [0_u32; MAX_ENTRIES];
    if status != 0 || size as usize > size_of_val(&objects) || !size.is_multiple_of(4) {
        return Err(DriverError::Backend(
            "owned HAL inventory exceeds transport capacity or cannot be read".into(),
        ));
    }
    let length = read(plugin, &property, &mut objects)
        .filter(|length| length.is_multiple_of(4))
        .ok_or_else(|| DriverError::Backend("owned HAL device-list readback failed".into()))?;
    objects[..length / 4]
        .iter()
        .map(|object| {
            let uid = string(*object, *b"uid ").ok_or_else(|| {
                DriverError::Backend("owned HAL device UID readback failed".into())
            })?;
            let id = endpoint_from_uid(&uid)
                .filter(|id| device(*id) == Some(*object))
                .ok_or_else(|| {
                    DriverError::Backend("owned HAL device identity is not canonical".into())
                })?;
            let name = string(*object, *b"lnam").ok_or_else(|| {
                DriverError::Backend("owned HAL device name readback failed".into())
            })?;
            let rate = rate(id).ok_or_else(|| {
                DriverError::Backend("owned HAL nominal-rate readback failed".into())
            })?;
            observed_config(
                id,
                name,
                rate,
                observed_stream(*object, *b"outp")?,
                observed_stream(*object, *b"inpt")?,
            )
        })
        .collect()
}
pub(super) fn rate(id: EndpointId) -> Option<u32> {
    let object = device(id)?;
    let mut rate = 0.0_f64;
    let mut size = 8;
    let property = address(*b"nsrt");
    // SAFETY: the property value is one f64; Core Audio receives its exact initialized
    // writable extent and does not retain this stack storage.
    let result = unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const property,
            0,
            ptr::null(),
            &raw mut size,
            (&raw mut rate).cast(),
        )
    };
    if result != 0 || size != 8 || !rate.is_finite() {
        return None;
    }
    mixrelay_audio_io::formats::SAMPLE_RATES
        .iter()
        .copied()
        .find(|value| f64::from(*value).to_bits() == rate.to_bits())
}
pub(super) fn supports(id: EndpointId, rate: u32) -> bool {
    if !mixrelay_audio_io::formats::SAMPLE_RATES.contains(&rate) {
        return false;
    }
    let Some(object) = device(id) else {
        return false;
    };
    let mut ranges = [Range::default(); 32];
    let mut size = u32::try_from(size_of_val(&ranges)).unwrap_or(0);
    let property = address(*b"nsr#");
    // SAFETY: the declared buffer capacity is the complete aligned Range array.
    // Returned byte length is checked before slicing; failed calls expose no values.
    let result = unsafe {
        AudioObjectGetPropertyData(
            object,
            &raw const property,
            0,
            ptr::null(),
            &raw mut size,
            ranges.as_mut_ptr().cast(),
        )
    };
    if result != 0
        || size as usize > size_of_val(&ranges)
        || !(size as usize).is_multiple_of(size_of::<Range>())
    {
        return false;
    }
    ranges[..size as usize / size_of::<Range>()]
        .iter()
        .any(|range| (range.minimum..=range.maximum).contains(&f64::from(rate)))
}
pub(super) fn set_rate(id: EndpointId, rate: u32) -> bool {
    if !supports(id, rate) {
        return false;
    }
    let Some(object) = device(id) else {
        return false;
    };
    let rate = f64::from(rate);
    let property = address(*b"nsrt");
    // SAFETY: Core Audio synchronously copies this finite supported f64 value;
    // no pointer is retained and no device outside the ID-derived UID is selected.
    unsafe {
        AudioObjectSetPropertyData(
            object,
            &raw const property,
            0,
            ptr::null(),
            8,
            (&raw const rate).cast(),
        ) == 0
    }
}

pub(super) fn set_format(id: EndpointId, render: bool, wanted: NativeStreamFormat) -> bool {
    let Some(object) = device(id) else {
        return false;
    };
    let scope = if render { *b"outp" } else { *b"inpt" };
    if !stream_formats(object, scope).supported.contains(&wanted) {
        return false;
    }
    let mut property = address(*b"stm#");
    property.scope = u32::from_be_bytes(scope);
    let mut stream = 0;
    if read(object, &property, &mut stream) != Some(4) || stream == 0 {
        return false;
    }
    let (flags, bits) = match wanted.sample_format {
        NativeSampleFormat::Float32 => (9, 32),
        NativeSampleFormat::Signed16 => (12, 16),
        NativeSampleFormat::Signed24 => (12, 24),
        NativeSampleFormat::Signed32 => (12, 32),
    };
    let bytes = u32::from(wanted.channels) * bits / 8;
    let asbd = Asbd {
        rate: f64::from(wanted.sample_rate),
        format: u32::from_be_bytes(*b"lpcm"),
        flags,
        packet_bytes: bytes,
        packet_frames: 1,
        frame_bytes: bytes,
        channels: u32::from(wanted.channels),
        bits,
        reserved: 0,
    };
    // SAFETY: the exact directional stream receives one initialized ASBD;
    // Core Audio copies it synchronously, and native readback confirms perform.
    unsafe {
        AudioObjectSetPropertyData(
            stream,
            &address(*b"pft "),
            0,
            ptr::null(),
            40,
            (&raw const asbd).cast(),
        ) == 0
    }
}

pub(super) struct CoreAudio;
impl crate::device_observation::DeviceProperties for CoreAudio {
    fn inventory(&self) -> Result<Vec<DriverDeviceConfig>, DriverError> {
        observed_inventory()
    }
    fn present(&self, id: EndpointId) -> bool {
        device(id).is_some()
    }
    fn rate(&self, id: EndpointId) -> Option<u32> {
        rate(id)
    }
    fn supports(&self, id: EndpointId, rate: u32) -> bool {
        supports(id, rate)
    }
    fn formats(&self, id: EndpointId) -> NativeDeviceFormats {
        native_formats(id)
    }
    fn set_rate(&self, id: EndpointId, rate: u32) -> bool {
        set_rate(id, rate)
    }
    fn set_format(&self, id: EndpointId, render: bool, format: NativeStreamFormat) -> bool {
        set_format(id, render, format)
    }
}

pub(super) struct InstalledBundle;
impl crate::device_observation::HalInstallation for InstalledBundle {
    fn installed(&self) -> bool {
        std::path::Path::new("/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver").is_dir()
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn observed_hal_uid_roundtrips_only_the_exact_own_canonical_identity() {
        let id = parse_endpoint_id("17e59d73-42c1-4f11-a8b4-6c2d913357e0").unwrap();
        assert_eq!(endpoint_from_uid(&endpoint_uid(id)), Some(id));
        for uid in [
            "io.other.audio.endpoint.17e59d7342c14f11a8b46c2d913357e0",
            "io.mixrelay.audio.endpoint.17E59D7342C14F11A8B46C2D913357E0",
            "io.mixrelay.audio.endpoint.00000000000000000000000000000000",
            "io.mixrelay.audio.endpoint.17e59d73-42c1-4f11-a8b4-6c2d913357e0",
            "io.mixrelay.audio.endpoint.🙂🙂🙂🙂🙂🙂🙂🙂",
        ] {
            assert_eq!(endpoint_from_uid(uid), None);
        }
    }

    #[test]
    fn startup_baseline_keeps_actual_name_rate_channels_and_directional_encodings() {
        let id = [7; 16];
        let render = NativeStreamFormat {
            sample_rate: 96_000,
            channels: 7,
            sample_format: NativeSampleFormat::Signed24,
        };
        let capture = NativeStreamFormat {
            sample_rate: 96_000,
            channels: 33,
            sample_format: NativeSampleFormat::Signed16,
        };
        let observed = observed_config(
            id,
            "Existing user device".into(),
            96_000,
            Some(render),
            Some(capture),
        )
        .unwrap();
        assert_eq!(observed.spec.id.as_str(), format_endpoint_id(id));
        assert_eq!(observed.spec.display_name, "Existing user device");
        assert_eq!(observed.spec.default_sample_rate, 96_000);
        assert_eq!(observed.spec.render.unwrap().channels, 7);
        assert_eq!(observed.spec.capture.unwrap().channels, 33);
        assert_eq!(observed.render_format, Some(render));
        assert_eq!(observed.capture_format, Some(capture));
        assert!(
            observed_config(id, "Existing".into(), 48_000, Some(render), Some(capture)).is_err()
        );
        assert!(observed_config(id, "Existing".into(), 96_000, None, None).is_err());
        let capture_only =
            observed_config(id, "Capture only".into(), 96_000, None, Some(capture)).unwrap();
        assert!(capture_only.spec.render.is_none());
        assert_eq!(capture_only.capture_format, Some(capture));
    }

    #[test]
    fn native_asbd_readback_recognizes_packed_depths_rates_and_geometry_only() {
        assert_eq!(size_of::<Asbd>(), 40);
        assert_eq!(size_of::<RangedFormat>(), 56);
        for &rate in mixrelay_audio_io::formats::SAMPLE_RATES {
            for channels in [1, 2, 7, 64] {
                for (flags, bits, sample_format) in [
                    (9, 32, NativeSampleFormat::Float32),
                    (12, 16, NativeSampleFormat::Signed16),
                    (12, 24, NativeSampleFormat::Signed24),
                    (12, 32, NativeSampleFormat::Signed32),
                ] {
                    let asbd = Asbd {
                        rate: f64::from(rate),
                        format: u32::from_be_bytes(*b"lpcm"),
                        flags,
                        packet_bytes: channels * bits / 8,
                        packet_frames: 1,
                        frame_bytes: channels * bits / 8,
                        channels,
                        bits,
                        reserved: 0,
                    };
                    assert_eq!(
                        parse_native(asbd),
                        Some(NativeStreamFormat {
                            sample_rate: rate,
                            channels: u16::try_from(channels).unwrap(),
                            sample_format,
                        })
                    );
                    assert_eq!(
                        parse_native(Asbd {
                            flags: flags | 2,
                            ..asbd
                        }),
                        None
                    );
                    assert_eq!(
                        parse_native(Asbd {
                            frame_bytes: asbd.frame_bytes + 1,
                            ..asbd
                        }),
                        None
                    );
                    assert_eq!(
                        parse_native(Asbd {
                            rate: 48000.5,
                            ..asbd
                        }),
                        None
                    );
                    assert_eq!(
                        parse_native(Asbd {
                            channels: 0,
                            ..asbd
                        }),
                        None
                    );
                }
            }
        }
    }
}
