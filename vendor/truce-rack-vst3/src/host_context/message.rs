//! Host-owned VST3 controller/editor message services.
//!
//! These control-plane interfaces may allocate and lock. They are intentionally separate from
//! `IComponentHandler` callbacks, which only enqueue edits through the atomic inbox.

use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    ptr,
    sync::Mutex,
};
use vst3::{
    Class, ComWrapper,
    Steinberg::{
        TUID,
        Vst::{
            IAttributeList, IAttributeList_iid, IAttributeListTrait, IMessage, IMessage_iid,
            IMessageTrait, TChar,
        },
        kInvalidArgument, kResultFalse, kResultOk,
    },
};

#[derive(Debug, PartialEq)]
enum AttributeValue {
    Int(i64),
    Float(f64),
    String(Vec<TChar>),
    Binary(Box<[u8]>),
}

#[derive(Default)]
struct HostAttributeList {
    values: Mutex<HashMap<Vec<u8>, AttributeValue>>,
}

impl Class for HostAttributeList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for HostAttributeList {
    unsafe fn setInt(&self, id: *const c_char, value: i64) -> i32 {
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(mut values) = self.values.lock() else {
            return kResultFalse;
        };
        let _ = values.insert(key, AttributeValue::Int(value));
        kResultOk
    }

    unsafe fn getInt(&self, id: *const c_char, value: *mut i64) -> i32 {
        if value.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(values) = self.values.lock() else {
            return kResultFalse;
        };
        let Some(AttributeValue::Int(current)) = values.get(&key) else {
            return kResultFalse;
        };
        // SAFETY: the VST3 caller supplied a writable output pointer; null was rejected above.
        unsafe { *value = *current };
        kResultOk
    }

    unsafe fn setFloat(&self, id: *const c_char, value: f64) -> i32 {
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(mut values) = self.values.lock() else {
            return kResultFalse;
        };
        let _ = values.insert(key, AttributeValue::Float(value));
        kResultOk
    }

    unsafe fn getFloat(&self, id: *const c_char, value: *mut f64) -> i32 {
        if value.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(values) = self.values.lock() else {
            return kResultFalse;
        };
        let Some(AttributeValue::Float(current)) = values.get(&key) else {
            return kResultFalse;
        };
        // SAFETY: the VST3 caller supplied a writable output pointer; null was rejected above.
        unsafe { *value = *current };
        kResultOk
    }

    unsafe fn setString(&self, id: *const c_char, string: *const TChar) -> i32 {
        if string.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let mut value = Vec::new();
        let mut index = 0_usize;
        loop {
            // SAFETY: VST3 supplies a readable NUL-terminated TChar string.
            let character = unsafe { string.add(index).read() };
            value.push(character);
            if character == 0 {
                break;
            }
            index += 1;
        }
        let Ok(mut values) = self.values.lock() else {
            return kResultFalse;
        };
        let _ = values.insert(key, AttributeValue::String(value));
        kResultOk
    }

    unsafe fn getString(&self, id: *const c_char, string: *mut TChar, size_in_bytes: u32) -> i32 {
        if string.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(values) = self.values.lock() else {
            return kResultFalse;
        };
        let Some(AttributeValue::String(value)) = values.get(&key) else {
            return kResultFalse;
        };
        let Some(source_bytes) = value.len().checked_mul(std::mem::size_of::<TChar>()) else {
            return kResultFalse;
        };
        let Ok(capacity) = usize::try_from(size_in_bytes) else {
            return kResultFalse;
        };
        let bytes_to_copy = source_bytes.min(capacity);
        if bytes_to_copy != 0 {
            // SAFETY: VST3 promises a writable buffer of `size_in_bytes` bytes. Copying the
            // minimum byte count preserves the SDK's truncating behavior, including odd sizes.
            unsafe {
                ptr::copy_nonoverlapping(
                    value.as_ptr().cast::<u8>(),
                    string.cast::<u8>(),
                    bytes_to_copy,
                );
            }
        }
        kResultOk
    }

    unsafe fn setBinary(&self, id: *const c_char, data: *const c_void, size_in_bytes: u32) -> i32 {
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(size) = usize::try_from(size_in_bytes) else {
            return kInvalidArgument;
        };
        if size > 0 && data.is_null() {
            return kInvalidArgument;
        }
        let bytes = if size == 0 {
            Box::default()
        } else {
            // SAFETY: VST3 supplies `size_in_bytes` readable bytes when size is nonzero.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) }
                .to_vec()
                .into_boxed_slice()
        };
        let Ok(mut values) = self.values.lock() else {
            return kResultFalse;
        };
        let _ = values.insert(key, AttributeValue::Binary(bytes));
        kResultOk
    }

    unsafe fn getBinary(
        &self,
        id: *const c_char,
        data: *mut *const c_void,
        size_in_bytes: *mut u32,
    ) -> i32 {
        if !data.is_null() {
            // SAFETY: non-null output pointer is writable by the VST3 caller contract.
            unsafe { *data = ptr::null() };
        }
        if !size_in_bytes.is_null() {
            // SAFETY: non-null output pointer is writable by the VST3 caller contract.
            unsafe { *size_in_bytes = 0 };
        }
        if data.is_null() || size_in_bytes.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { attribute_key(id) }) else {
            return kInvalidArgument;
        };
        let Ok(values) = self.values.lock() else {
            return kResultFalse;
        };
        let Some(AttributeValue::Binary(bytes)) = values.get(&key) else {
            return kResultFalse;
        };
        let Ok(size) = u32::try_from(bytes.len()) else {
            return kResultFalse;
        };
        // SAFETY: both outputs were checked above. Binary allocation remains stable through map
        // rehashes and valid until this attribute is replaced or the list is destroyed.
        unsafe {
            *data = if bytes.is_empty() {
                ptr::null()
            } else {
                bytes.as_ptr().cast::<c_void>()
            };
            *size_in_bytes = size;
        }
        kResultOk
    }
}

unsafe fn attribute_key(id: *const c_char) -> Option<Vec<u8>> {
    if id.is_null() {
        return None;
    }
    // SAFETY: `id` is a VST3 AttrID, which is a valid NUL-terminated C string.
    Some(unsafe { CStr::from_ptr(id) }.to_bytes().to_vec())
}

struct HostMessage {
    id: Mutex<Option<CString>>,
    attributes: ComWrapper<HostAttributeList>,
}

impl HostMessage {
    fn new() -> Self {
        Self {
            id: Mutex::new(None),
            attributes: ComWrapper::new(HostAttributeList::default()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> *const c_char {
        match self.id.lock() {
            Ok(id) => id.as_ref().map_or(ptr::null(), |id| id.as_ptr()),
            Err(_) => ptr::null(),
        }
    }

    unsafe fn setMessageID(&self, id: *const c_char) {
        let id = if id.is_null() {
            None
        } else {
            // SAFETY: VST3 supplies a valid NUL-terminated FIDString.
            Some(unsafe { CStr::from_ptr(id) }.to_owned())
        };
        if let Ok(mut current) = self.id.lock() {
            *current = id;
        }
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes
            .as_com_ref::<IAttributeList>()
            .map_or(ptr::null_mut(), |attributes| attributes.as_ptr())
    }
}

/// Creates one supported host control-plane COM interface and transfers its reference to caller.
///
/// The caller owns the returned reference and must release it through the matching COM interface.
pub(super) fn create_instance(class_id: &TUID, interface_id: &TUID) -> Option<*mut c_void> {
    if *class_id == IMessage_iid && *interface_id == IMessage_iid {
        let wrapper = ComWrapper::new(HostMessage::new());
        return wrapper
            .to_com_ptr::<IMessage>()
            .map(|message| message.into_raw().cast::<c_void>());
    }
    if *class_id == IAttributeList_iid && *interface_id == IAttributeList_iid {
        let wrapper = ComWrapper::new(HostAttributeList::default());
        return wrapper
            .to_com_ptr::<IAttributeList>()
            .map(|attributes| attributes.into_raw().cast::<c_void>());
    }
    None
}
