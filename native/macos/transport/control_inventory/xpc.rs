//! Typed access to the libxpc objects every control message is built from.
use super::NAME_BYTES;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
};

pub(crate) type Xpc = *mut c_void;
pub(super) type XpcType = *const c_void;

unsafe extern "C" {
    pub(super) static _xpc_type_dictionary: c_char;
    pub(super) static _xpc_type_array: c_char;
    pub(super) static _xpc_type_uint64: c_char;
    pub(super) static _xpc_type_string: c_char;
    pub(super) static _xpc_type_data: c_char;
    pub(super) static _xpc_type_fd: c_char;
    pub(super) fn xpc_get_type(value: Xpc) -> XpcType;
    pub(super) fn xpc_release(value: Xpc);
    pub(super) fn xpc_dictionary_create(
        keys: *const *const c_char,
        values: *const Xpc,
        count: usize,
    ) -> Xpc;
    pub(crate) fn xpc_dictionary_create_reply(request: Xpc) -> Xpc;
    pub(super) fn xpc_dictionary_get_value(dictionary: Xpc, key: *const c_char) -> Xpc;
    pub(super) fn xpc_dictionary_set_value(dictionary: Xpc, key: *const c_char, value: Xpc);
    pub(crate) fn xpc_dictionary_set_uint64(dictionary: Xpc, key: *const c_char, value: u64);
    pub(crate) fn xpc_dictionary_set_string(
        dictionary: Xpc,
        key: *const c_char,
        value: *const c_char,
    );
    pub(super) fn xpc_dictionary_set_data(
        dictionary: Xpc,
        key: *const c_char,
        value: *const c_void,
        length: usize,
    );
    pub(crate) fn xpc_dictionary_set_fd(dictionary: Xpc, key: *const c_char, descriptor: c_int);
    pub(crate) fn xpc_dictionary_dup_fd(dictionary: Xpc, key: *const c_char) -> c_int;
    pub(super) fn xpc_uint64_get_value(value: Xpc) -> u64;
    pub(super) fn xpc_string_get_string_ptr(value: Xpc) -> *const c_char;
    pub(super) fn xpc_string_get_length(value: Xpc) -> usize;
    pub(super) fn xpc_data_get_bytes_ptr(value: Xpc) -> *const c_void;
    pub(super) fn xpc_data_get_length(value: Xpc) -> usize;
    pub(super) fn xpc_string_create(value: *const c_char) -> Xpc;
    pub(super) fn xpc_array_create(values: *const Xpc, count: usize) -> Xpc;
    pub(super) fn xpc_array_append_value(array: Xpc, value: Xpc);
    pub(super) fn xpc_array_get_count(array: Xpc) -> usize;
    pub(super) fn xpc_array_get_value(array: Xpc, index: usize) -> Xpc;
}

pub(crate) struct Message(pub(crate) Xpc);
impl Message {
    pub(crate) fn new(raw: Xpc) -> Option<Self> {
        if raw.is_null() { None } else { Some(Self(raw)) }
    }
    pub(crate) fn into_raw(self) -> Xpc {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}
impl Drop for Message {
    fn drop(&mut self) {
        unsafe { xpc_release(self.0) };
    }
}

pub(super) fn is_type(value: Xpc, expected: XpcType) -> bool {
    !value.is_null() && unsafe { xpc_get_type(value) } == expected
}
pub(super) fn dictionary(value: Xpc) -> bool {
    is_type(value, (&raw const _xpc_type_dictionary).cast())
}
pub(super) fn field(message: Xpc, key: &CStr) -> Xpc {
    unsafe { xpc_dictionary_get_value(message, key.as_ptr()) }
}
pub(crate) fn uint(message: Xpc, key: &CStr) -> Option<u64> {
    let value = field(message, key);
    is_type(value, (&raw const _xpc_type_uint64).cast())
        .then(|| unsafe { xpc_uint64_get_value(value) })
}
pub(super) fn string(value: Xpc) -> Option<CString> {
    if !is_type(value, (&raw const _xpc_type_string).cast()) {
        return None;
    }
    let length = unsafe { xpc_string_get_length(value) };
    if length >= NAME_BYTES {
        return None;
    }
    let pointer = unsafe { xpc_string_get_string_ptr(value) };
    if pointer.is_null() {
        return None;
    }
    // The XPC string owns length+1 bytes. Copy the bounded view so no borrow can
    // outlive its parent message, including during an early validation return.
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length + 1) };
    let value = CStr::from_bytes_with_nul(bytes).ok()?;
    value.to_str().ok()?;
    Some(value.to_owned())
}
pub(crate) fn operation(message: Xpc, expected: &CStr) -> bool {
    dictionary(message)
        && uint(message, c"version") == Some(1)
        && string(field(message, c"operation")).as_deref() == Some(expected)
}
pub(crate) fn new_message(operation: &CStr) -> Option<Message> {
    let result = Message::new(unsafe { xpc_dictionary_create(ptr::null(), ptr::null(), 0) })?;
    unsafe {
        xpc_dictionary_set_uint64(result.0, c"version".as_ptr(), 1);
        xpc_dictionary_set_string(result.0, c"operation".as_ptr(), operation.as_ptr());
    }
    Some(result)
}
pub(super) fn endpoint_id(message: Xpc) -> Option<[u8; 16]> {
    let value = field(message, c"endpoint_id");
    if !is_type(value, (&raw const _xpc_type_data).cast())
        || unsafe { xpc_data_get_length(value) } != 16
    {
        return None;
    }
    let pointer = unsafe { xpc_data_get_bytes_ptr(value) };
    if pointer.is_null() {
        return None;
    }
    let mut id = [0; 16];
    id.copy_from_slice(unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), 16) });
    id.iter().any(|byte| *byte != 0).then_some(id)
}
