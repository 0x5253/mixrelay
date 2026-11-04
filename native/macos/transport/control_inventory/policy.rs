//! Transport presentation policy: the bundle identifiers a publishing service admits,
//! carried in every inventory snapshot.
use super::xpc::{
    _xpc_type_array, Message, Xpc, field, is_type, string, xpc_array_append_value,
    xpc_array_create, xpc_array_get_count, xpc_array_get_value, xpc_dictionary_set_value,
    xpc_string_create,
};
use super::{MAX_TRANSPORT_BUNDLES, NAME_BYTES};
use std::{
    ffi::{CStr, CString, c_char},
    ptr,
};

/// Immutable transport presentation policy owned by one publishing service.
#[derive(Debug, Clone, Default)]
pub struct ProducerPolicy(pub(super) Vec<CString>);

impl ProducerPolicy {
    /// Rejects malformed, duplicate or excessive bundle identifiers before publication.
    #[must_use]
    pub fn new(bundles: &[String]) -> Option<Self> {
        if bundles.len() > MAX_TRANSPORT_BUNDLES
            || bundles
                .iter()
                .enumerate()
                .any(|(index, bundle)| !valid_bundle(bundle) || bundles[..index].contains(bundle))
        {
            return None;
        }
        bundles
            .iter()
            .map(|bundle| CString::new(bundle.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .map(Self)
    }
}

fn valid_bundle(bundle: &str) -> bool {
    !bundle.is_empty()
        && bundle.len() < NAME_BYTES
        && bundle.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TransportCompatibility {
    pub count: usize,
    pub bundles: [[c_char; NAME_BYTES]; MAX_TRANSPORT_BUNDLES],
}
impl Default for TransportCompatibility {
    fn default() -> Self {
        Self {
            count: 0,
            bundles: [[0; NAME_BYTES]; MAX_TRANSPORT_BUNDLES],
        }
    }
}
impl TransportCompatibility {
    /// Returns the bounded, terminated identifiers copied by the decoder.
    #[must_use]
    pub fn identifiers(&self) -> Vec<String> {
        self.bundles
            .iter()
            .take(self.count)
            .filter_map(|bytes| {
                let bytes =
                    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), NAME_BYTES) };
                CStr::from_bytes_until_nul(bytes)
                    .ok()?
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            })
            .collect()
    }
}
pub(super) fn write_policy(message: Xpc, bundles: &[CString]) -> Option<()> {
    let array = Message::new(unsafe { xpc_array_create(ptr::null(), 0) })?;
    for bundle in bundles {
        let item = Message::new(unsafe { xpc_string_create(bundle.as_ptr()) })?;
        unsafe { xpc_array_append_value(array.0, item.0) };
    }
    unsafe { xpc_dictionary_set_value(message, c"transport_compatibility".as_ptr(), array.0) };
    Some(())
}
pub(super) fn parse_policy(message: Xpc) -> Option<TransportCompatibility> {
    let value = field(message, c"transport_compatibility");
    let mut result = TransportCompatibility::default();
    if !is_type(value, (&raw const _xpc_type_array).cast()) {
        return None;
    }
    let count = unsafe { xpc_array_get_count(value) };
    if count > MAX_TRANSPORT_BUNDLES {
        return None;
    }
    let mut seen = Vec::with_capacity(count);
    for index in 0..count {
        let bundle = string(unsafe { xpc_array_get_value(value, index) })?;
        if !valid_bundle(bundle.to_str().ok()?) || seen.contains(&bundle) {
            return None;
        }
        for (target, byte) in result.bundles[index]
            .iter_mut()
            .zip(bundle.to_bytes_with_nul())
        {
            *target = c_char::from_ne_bytes([*byte]);
        }
        seen.push(bundle);
    }
    result.count = count;
    Some(result)
}
