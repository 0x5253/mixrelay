//! All-or-nothing control snapshots of VM grants; no parsing occurs on callbacks.
mod endpoints;
use super::{ClientPcmReader, ClientPcmWriter, mapping::Object};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, c_char, c_void},
    fmt, ptr,
};

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    static _xpc_type_dictionary: c_char;
    static _xpc_type_array: c_char;
    static _xpc_type_shmem: c_char;
    fn xpc_get_type(object: Object) -> *const c_void;
    fn xpc_release(object: Object);
    fn xpc_dictionary_create(
        keys: *const *const c_char,
        values: *const Object,
        count: usize,
    ) -> Object;
    fn xpc_dictionary_set_string(object: Object, key: *const c_char, value: *const c_char);
    fn xpc_dictionary_get_string(object: Object, key: *const c_char) -> *const c_char;
    fn xpc_dictionary_set_uint64(object: Object, key: *const c_char, value: u64);
    fn xpc_dictionary_get_uint64(object: Object, key: *const c_char) -> u64;
    fn xpc_dictionary_set_value(object: Object, key: *const c_char, value: Object);
    fn xpc_dictionary_get_value(object: Object, key: *const c_char) -> Object;
    fn xpc_array_create(values: *const Object, count: usize) -> Object;
    fn xpc_array_append_value(array: Object, value: Object);
    fn xpc_array_get_count(array: Object) -> usize;
    fn xpc_array_get_value(array: Object, index: usize) -> Object;
}

/// Owns the complete control message and its retained VM grants.
#[derive(Debug)]
pub struct ClientPcmSnapshot(Object);
impl ClientPcmSnapshot {
    #[must_use]
    pub fn new() -> Option<Self> {
        // SAFETY: all keys and values are copied into these newly created objects.
        unsafe {
            let object = xpc_dictionary_create(ptr::null(), ptr::null(), 0);
            if object.is_null() {
                return None;
            }
            let snapshot = Self(object);
            xpc_dictionary_set_uint64(object, c"observed_at".as_ptr(), mach_absolute_time());
            let array = xpc_array_create(ptr::null(), 0);
            if array.is_null() {
                return None;
            }
            xpc_dictionary_set_string(
                object,
                c"operation".as_ptr(),
                c"virtual_pcm_snapshot".as_ptr(),
            );
            xpc_dictionary_set_uint64(object, c"version".as_ptr(), 2);
            xpc_dictionary_set_value(object, c"queues".as_ptr(), array);
            xpc_release(array);
            let endpoints = xpc_array_create(ptr::null(), 0);
            if endpoints.is_null() {
                return None;
            }
            xpc_dictionary_set_value(object, c"endpoints".as_ptr(), endpoints);
            xpc_release(endpoints);
            Some(snapshot)
        }
    }
    /// # Safety
    /// A live snapshot dictionary validated by `decode`.
    pub unsafe fn observed_at(message: Object) -> u64 {
        // SAFETY: the caller retains the decoded dictionary.
        unsafe { xpc_dictionary_get_uint64(message, c"observed_at".as_ptr()) }
    }
    #[must_use]
    pub fn as_raw(&self) -> Object {
        self.0
    }
    pub fn push(&mut self, writer: &ClientPcmWriter) -> bool {
        if !self.push_endpoint(super::EndpointGeometry {
            id: writer.geometry().id,
            render_channels: writer.geometry().render_channels,
            capture_channels: writer.geometry().capture_channels,
        }) {
            return false;
        }
        // SAFETY: this exclusive snapshot owns the array and writer retains its grant.
        unsafe {
            let array = xpc_dictionary_get_value(self.0, c"queues".as_ptr());
            if xpc_array_get_count(array)
                >= crate::control_inventory::MAX_ENTRIES * super::MAX_CLIENTS_PER_ENDPOINT
            {
                return false;
            }
            let entry = xpc_dictionary_create(ptr::null(), ptr::null(), 0);
            if entry.is_null() {
                return false;
            }
            xpc_dictionary_set_value(entry, c"pcm".as_ptr(), writer.grant());
            xpc_dictionary_set_value(entry, c"clock".as_ptr(), writer.clock_grant());
            xpc_dictionary_set_uint64(entry, c"membership".as_ptr(), writer.membership());
            xpc_array_append_value(array, entry);
            xpc_release(entry);
        }
        true
    }
    /// # Errors
    /// Rejects malformed grants, duplicate clients and excess mapped PCM.
    /// # Safety
    /// Message is null or a live borrowed XPC object throughout this call.
    /// Grants come from a trusted HAL producer that keeps geometry immutable;
    /// the caller authenticates that peer before decoding shared VM contents.
    pub unsafe fn decode(message: Object) -> Result<Vec<ClientPcmReader>, SnapshotError> {
        // SAFETY: XPC object type is validated before dictionary/array access.
        unsafe {
            if message.is_null()
                || xpc_get_type(message) != (&raw const _xpc_type_dictionary).cast()
            {
                return Err(SnapshotError::Invalid);
            }
            let operation = xpc_dictionary_get_string(message, c"operation".as_ptr());
            if operation.is_null()
                || CStr::from_ptr(operation) != c"virtual_pcm_snapshot"
                || xpc_dictionary_get_uint64(message, c"version".as_ptr()) != 2
            {
                return Err(SnapshotError::Invalid);
            }
            let array = xpc_dictionary_get_value(message, c"queues".as_ptr());
            if array.is_null() || xpc_get_type(array) != (&raw const _xpc_type_array).cast() {
                return Err(SnapshotError::Invalid);
            }
            let endpoints = Self::endpoints(message)?;
            let queues = Self::decode_queues(array)?;
            if queues.iter().any(|queue| {
                !endpoints.iter().any(|shape| {
                    shape.id == queue.endpoint()
                        && shape.render_channels == queue.channels()
                        && shape.capture_channels == queue.capture_channels()
                })
            }) {
                return Err(SnapshotError::Invalid);
            }
            Ok(queues)
        }
    }
    unsafe fn decode_queues(array: Object) -> Result<Vec<ClientPcmReader>, SnapshotError> {
        // SAFETY: decode validated the live array's type and retains its parent message.
        let count = unsafe { xpc_array_get_count(array) };
        if count > crate::control_inventory::MAX_ENTRIES * super::MAX_CLIENTS_PER_ENDPOINT {
            return Err(SnapshotError::Limit);
        }
        let mut queues = Vec::new();
        queues
            .try_reserve_exact(count)
            .map_err(|_| SnapshotError::Limit)?;
        let mut ids = BTreeSet::new();
        let mut endpoints = BTreeMap::new();
        for index in 0..count {
            // SAFETY: index is within the array retained by the decoder's caller.
            let entry = unsafe { xpc_array_get_value(array, index) };
            if entry.is_null()
                || unsafe { xpc_get_type(entry) } != (&raw const _xpc_type_dictionary).cast()
            {
                return Err(SnapshotError::Invalid);
            }
            // SAFETY: the typed dictionary and both borrowed values remain retained by message.
            let grant = unsafe { xpc_dictionary_get_value(entry, c"pcm".as_ptr()) };
            let clock = unsafe { xpc_dictionary_get_value(entry, c"clock".as_ptr()) };
            if [grant, clock].iter().any(|grant| {
                grant.is_null()
                    || unsafe { xpc_get_type(*grant) } != (&raw const _xpc_type_shmem).cast()
            }) {
                return Err(SnapshotError::Invalid);
            }
            // SAFETY: both grants have validated types and immutable HAL-owned headers.
            let membership = unsafe { xpc_dictionary_get_uint64(entry, c"membership".as_ptr()) };
            let queue =
                unsafe { ClientPcmReader::attach(grant, clock, membership) }.map_err(|source| {
                    SnapshotError::Mapping {
                        queue: index,
                        source,
                    }
                })?;
            let (clients, bytes) = endpoints
                .entry(queue.endpoint())
                .or_insert((0_usize, 0_usize));
            *clients += 1;
            let required = crate::shared_audio_buffer::mr_shared_audio_buffer_required_bytes(
                queue.capacity(),
                queue.channels(),
            );
            *bytes = bytes
                .checked_add(required)
                .filter(|bytes| *bytes <= super::MAX_PCM_BYTES_PER_ENDPOINT)
                .ok_or(SnapshotError::Limit)?;
            if *clients > super::MAX_CLIENTS_PER_ENDPOINT
                || endpoints.len() > crate::control_inventory::MAX_ENTRIES
            {
                return Err(SnapshotError::Limit);
            }
            if !ids.insert((queue.endpoint(), queue.client())) {
                return Err(SnapshotError::Invalid);
            }
            queues.push(queue);
        }
        Ok(queues)
    }
}
impl Drop for ClientPcmSnapshot {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns exactly one created XPC reference, released on control.
        unsafe { xpc_release(self.0) };
    }
}

#[derive(Debug)]
pub enum SnapshotError {
    Invalid,
    Limit,
    Mapping {
        queue: usize,
        source: std::io::Error,
    },
}
impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("invalid HAL PCM snapshot envelope or grant type"),
            Self::Limit => {
                formatter.write_str("HAL PCM snapshot exceeds endpoint admission bounds")
            }
            Self::Mapping { queue, source } => write!(
                formatter,
                "HAL PCM queue {queue} attachment failed: {source}"
            ),
        }
    }
}
impl std::error::Error for SnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Mapping { source, .. } = self {
            Some(source)
        } else {
            None
        }
    }
}
