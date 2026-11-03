use std::{
    ffi::{CStr, c_char, c_void},
    ptr::{self, NonNull},
};

use super::Selection;
use crate::{
    aggregate_group::{NativeBinding, mr_aggregate_group_binding, mr_aggregate_group_ready},
    device_buffers::{BufferSelection, NativeFormats},
    device_io::stream_format::{StreamFormat, StreamFormats},
};

const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const STREAMS: u32 = u32::from_be_bytes(*b"stm#");
const BUFFER_SIZE: u32 = u32::from_be_bytes(*b"fsiz");

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Address {
    pub(super) selector: u32,
    scope: u32,
    element: u32,
}

impl Address {
    pub(super) fn new(selector: u32, scope: u32) -> Self {
        Self {
            selector,
            scope,
            element: 0,
        }
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub(super) struct Description {
    pub(super) rate: f64,
    pub(super) format: u32,
    pub(super) flags: u32,
    pub(super) packet_bytes: u32,
    pub(super) packet_frames: u32,
    pub(super) frame_bytes: u32,
    pub(super) channels: u32,
    pub(super) bits: u32,
    pub(super) reserved: u32,
}

pub(super) struct StreamIds {
    pub(super) values: [u32; 2048],
    pub(super) count: usize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ValueRange {
    minimum: f64,
    maximum: f64,
}

/// Control-side device buffer facts. Missing optional bounds/settable facts stay zero.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct BufferInfo {
    /// Resolved native object identity.
    pub device: u32,
    /// Current frame size; zero means unavailable.
    pub frames: u32,
    /// Native lower bound, when available.
    pub minimum: f64,
    /// Native upper bound, when available.
    pub maximum: f64,
    /// One when `CoreAudio` reports the frame-size property writable.
    pub settable: u32,
}

type IoProc = unsafe extern "C" fn(
    u32,
    *const c_void,
    *const c_void,
    *const c_void,
    *mut c_void,
    *const c_void,
    *mut c_void,
) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    #[link_name = "AudioObjectGetPropertyData"]
    pub(super) fn property_read(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        output: *mut c_void,
    ) -> i32;
    #[link_name = "AudioObjectGetPropertyDataSize"]
    fn property_size(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    #[link_name = "AudioObjectSetPropertyData"]
    pub(super) fn property_write(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        input: *const c_void,
    ) -> i32;
    #[link_name = "AudioObjectIsPropertySettable"]
    fn property_settable(id: u32, address: *const Address, result: *mut u8) -> i32;
    #[link_name = "AudioDeviceCreateIOProcID"]
    pub(super) fn create(
        device: u32,
        callback: IoProc,
        context: *mut c_void,
        procedure: *mut *mut c_void,
    ) -> i32;
    #[link_name = "AudioDeviceStart"]
    pub(super) fn start(device: u32, procedure: *mut c_void) -> i32;
    #[link_name = "AudioDeviceStop"]
    pub(super) fn stop(device: u32, procedure: *mut c_void) -> i32;
    #[link_name = "AudioDeviceDestroyIOProcID"]
    pub(super) fn destroy(device: u32, procedure: *mut c_void) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    #[link_name = "CFStringCreateWithCString"]
    fn string_create(allocator: *const c_void, text: *const c_char, encoding: u32)
    -> *const c_void;
    #[link_name = "CFRelease"]
    fn release(value: *const c_void);
}

/// Private synchronous property boundary. Tests supply fixed scalar facts; no
/// callback or resource owner retains this adapter or its query-local output.
pub(super) trait Properties {
    fn read<T: Copy + Default>(&self, id: u32, address: Address) -> Option<T>;
    fn streams(&self, id: u32, scope: u32, limit: u32) -> Option<StreamIds>;
    /// The device a persistent UID currently names. The default resolves nothing, so an
    /// adapter of fixed scalar facts never reaches an installed device.
    fn device(&self, _uid: &CStr) -> Option<u32> {
        None
    }
    /// A private group member's checked projection; the default proves no group.
    ///
    /// # Safety
    /// `group` is a live aggregate owner retained, unmutated, until this query returns.
    unsafe fn group_binding(
        &self,
        _group: NonNull<c_void>,
        _uid: &CStr,
        _input: bool,
        _channels: u32,
    ) -> Option<NativeBinding> {
        None
    }
    /// Whether a private group still has its planned geometry; the default proves none.
    ///
    /// # Safety
    /// Same retained-owner contract as [`Properties::group_binding`].
    unsafe fn group_ready(&self, _group: NonNull<c_void>) -> bool {
        false
    }
    /// Whether a property accepts writes; the default reports read-only.
    fn settable(&self, _id: u32, _address: Address) -> bool {
        false
    }
    /// Writes one u32 property; the default rejects every write.
    fn write(&self, _id: u32, _address: Address, _value: u32) -> bool {
        false
    }
}

pub(super) struct System;

impl Properties for System {
    fn device(&self, uid: &CStr) -> Option<u32> {
        // SAFETY: the borrowed UID is terminated and alive for the call; CF returns +1.
        let uid = unsafe { string_create(ptr::null(), uid.as_ptr(), 0x0800_0100) };
        if uid.is_null() {
            return None;
        }
        let address = Address::new(u32::from_be_bytes(*b"uidd"), GLOBAL);
        let mut device = 0;
        let mut size = 4;
        // SAFETY: native qualifier is one retained CFStringRef; output is one u32.
        let status = unsafe {
            property_read(
                1,
                &raw const address,
                8,
                ptr::from_ref(&uid).cast(),
                &raw mut size,
                ptr::from_mut(&mut device).cast(),
            )
        };
        // SAFETY: this function owns exactly the create-rule CF reference.
        unsafe { release(uid) };
        (status == 0 && size == 4 && device != 0).then_some(device)
    }

    unsafe fn group_binding(
        &self,
        group: NonNull<c_void>,
        uid: &CStr,
        input: bool,
        channels: u32,
    ) -> Option<NativeBinding> {
        let mut binding = NativeBinding::default();
        // SAFETY: the caller retains the aggregate owner; the UID is terminated and
        // borrowed for the call, and `binding` is exclusive local storage.
        unsafe {
            mr_aggregate_group_binding(
                group.as_ptr().cast(),
                uid.as_ptr(),
                input,
                channels,
                &raw mut binding,
            )
        }
        .then_some(binding)
    }

    unsafe fn group_ready(&self, group: NonNull<c_void>) -> bool {
        // SAFETY: the caller retains the aggregate owner throughout this query.
        unsafe { mr_aggregate_group_ready(group.as_ptr().cast()) }
    }

    fn settable(&self, id: u32, address: Address) -> bool {
        let mut settable = 0;
        // SAFETY: CoreAudio Boolean is one byte, exclusively borrowed until return.
        let status = unsafe { property_settable(id, &raw const address, &raw mut settable) };
        status == 0 && settable != 0
    }

    fn write(&self, id: u32, address: Address, value: u32) -> bool {
        // SAFETY: a synchronous u32 property write borrows one immutable value.
        unsafe {
            property_write(
                id,
                &raw const address,
                0,
                ptr::null(),
                4,
                ptr::from_ref(&value).cast(),
            ) == 0
        }
    }

    fn read<T: Copy + Default>(&self, id: u32, address: Address) -> Option<T> {
        let mut result = T::default();
        let capacity = u32::try_from(size_of::<T>()).ok()?;
        let mut size = capacity;
        // SAFETY: all private call sites pair a native scalar ABI type with its
        // matching selector. Output owns exactly the advertised capacity.
        let status = unsafe {
            property_read(
                id,
                &raw const address,
                0,
                ptr::null(),
                &raw mut size,
                ptr::from_mut(&mut result).cast(),
            )
        };
        (status == 0 && size == capacity).then_some(result)
    }

    fn streams(&self, id: u32, scope: u32, limit: u32) -> Option<StreamIds> {
        if !matches!(limit, 64 | 2048) {
            return None;
        }
        let address = Address::new(STREAMS, scope);
        let mut size = 0;
        // SAFETY: scalar size output belongs to this control-side query.
        if unsafe { property_size(id, &raw const address, 0, ptr::null(), &raw mut size) } != 0
            || !valid_stream_extent(size, limit * 4)
        {
            return None;
        }
        let capacity = size;
        let mut streams = StreamIds {
            values: [0; 2048],
            count: 0,
        };
        if size == 0 {
            return Some(streams);
        }
        // SAFETY: initialized u32 buffer has the queried capacity; CoreAudio may
        // shorten the list, but returned size must never expand that capacity.
        let status = unsafe {
            property_read(
                id,
                &raw const address,
                0,
                ptr::null(),
                &raw mut size,
                streams.values.as_mut_ptr().cast(),
            )
        };
        if status != 0 || !valid_stream_extent(size, capacity) {
            return None;
        }
        streams.count = size as usize / 4;
        Some(streams)
    }
}

pub(super) fn valid_stream_extent(size: u32, capacity: u32) -> bool {
    size <= capacity && size.is_multiple_of(4)
}

pub(super) fn default_device(input: bool, properties: &impl Properties) -> Option<u32> {
    let selector = if input { *b"dIn " } else { *b"dOut" };
    properties.read(1, Address::new(u32::from_be_bytes(selector), GLOBAL))
}

pub(super) fn resolve(
    selection: &Selection,
    properties: &impl Properties,
) -> Option<NativeBinding> {
    let uid = CStr::from_bytes_with_nul(&selection.uid).ok()?;
    if let Some(group) = NonNull::new(selection.group) {
        // SAFETY: enclosing engine Native owns the aggregate Arc, retained until
        // this device context is destroyed.
        return unsafe {
            properties.group_binding(group, uid, selection.input, selection.channels)
        };
    }
    let device = if selection.uid.len() == 1 {
        default_device(selection.input, properties)?
    } else {
        properties.device(uid)?
    };
    (device != 0).then_some(NativeBinding {
        device,
        offset: 0,
        total: 0,
    })
}

#[cfg(test)]
pub(super) fn format(
    selection: &Selection,
    binding: NativeBinding,
    properties: &impl Properties,
) -> Option<u32> {
    admitted_format(selection, binding, properties).map(|format| format.rate)
}

pub(super) struct AdmittedFormat {
    pub(super) rate: u32,
    pub(super) layout: NativeFormats,
    /// The owner's channel slice of that layout.
    pub(super) selection: BufferSelection,
}

/// Queries and validates the complete native PCM layout before `IOProc` registration.
/// Missing properties or invalid packet/stream/selection geometry return `None`.
pub(super) fn admitted_format(
    selection: &Selection,
    binding: NativeBinding,
    properties: &impl Properties,
) -> Option<AdmittedFormat> {
    let slice = buffer_selection(selection, binding);
    let scope = u32::from_be_bytes(if selection.input { *b"inpt" } else { *b"outp" });
    let streams = properties.streams(binding.device, scope, slice.limit)?;
    let mut formats = StreamFormats::default();
    for &stream in &streams.values[..streams.count] {
        let format: Description =
            properties.read(stream, Address::new(u32::from_be_bytes(*b"sfmt"), GLOBAL))?;
        if format.packet_frames != 1 || format.packet_bytes != format.frame_bytes {
            return None;
        }
        if !formats.accept(
            &StreamFormat {
                rate: format.rate,
                format: format.format,
                flags: format.flags,
                bits: format.bits,
                channels: format.channels,
                frame_bytes: format.frame_bytes,
            },
            slice.limit,
        ) {
            return None;
        }
    }
    let rate = admitted_rate(formats.selected_rate(slice)?);
    Some(AdmittedFormat {
        rate,
        layout: formats.into_layout(),
        selection: slice,
    })
}

// Called only with StreamFormats' admitted finite integral 8000..384000 Hz value.
// Safe numeric conversion avoids introducing unsafe solely to satisfy cast lints.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "StreamFormats admitted finite integral 8000..384000 Hz"
)]
fn admitted_rate(rate: f64) -> u32 {
    rate as u32
}

pub(super) fn buffer_selection(selection: &Selection, binding: NativeBinding) -> BufferSelection {
    BufferSelection {
        channels: selection.channels,
        offset: binding.offset,
        total: if selection.group.is_null() {
            0
        } else {
            binding.total
        },
        limit: if selection.group.is_null() { 64 } else { 2048 },
    }
}

pub(super) fn buffer_size(device: u32, properties: &impl Properties) -> u32 {
    properties
        .read(device, Address::new(BUFFER_SIZE, GLOBAL))
        .unwrap_or(0)
}

pub(super) fn buffer_info(
    selection: &Selection,
    properties: &impl Properties,
) -> Option<BufferInfo> {
    let device = resolve(selection, properties)?.device;
    let frames = buffer_size(device, properties);
    if frames == 0 {
        return None;
    }
    let range: ValueRange = properties
        .read(device, Address::new(u32::from_be_bytes(*b"fsz#"), GLOBAL))
        .unwrap_or_default();
    Some(BufferInfo {
        device,
        frames,
        minimum: range.minimum,
        maximum: range.maximum,
        settable: u32::from(properties.settable(device, Address::new(BUFFER_SIZE, GLOBAL))),
    })
}

pub(super) fn buffer_write(device: u32, frames: u32, properties: &impl Properties) -> bool {
    properties.write(device, Address::new(BUFFER_SIZE, GLOBAL), frames)
}

pub(super) fn alive(device: u32, properties: &impl Properties) -> bool {
    properties
        .read::<u32>(device, Address::new(u32::from_be_bytes(*b"livn"), GLOBAL))
        .is_some_and(|alive| alive != 0)
}

pub(super) fn clock_device(device: u32, properties: &impl Properties) -> u32 {
    properties
        .read::<u32>(device, Address::new(u32::from_be_bytes(*b"tran"), GLOBAL))
        .filter(|transport| *transport != 0 && *transport != u32::from_be_bytes(*b"grup"))
        .map_or(0, |_| device)
}

const _: () = assert!(size_of::<BufferInfo>() == 32 && size_of::<Description>() == 40);
