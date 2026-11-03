//! Device inventory accompanies grants, including devices with no clients.
use super::{
    _xpc_type_array, _xpc_type_dictionary, ClientPcmSnapshot, Object, SnapshotError, c_char,
    c_void, ptr, xpc_array_append_value, xpc_array_get_count, xpc_array_get_value,
    xpc_dictionary_create, xpc_dictionary_get_uint64, xpc_dictionary_get_value,
    xpc_dictionary_set_uint64, xpc_get_type, xpc_release,
};
use crate::client_pcm::EndpointGeometry;

unsafe extern "C" {
    fn xpc_dictionary_set_data(
        object: Object,
        key: *const c_char,
        bytes: *const c_void,
        count: usize,
    );
    fn xpc_dictionary_get_data(
        object: Object,
        key: *const c_char,
        count: *mut usize,
    ) -> *const c_void;
}

impl ClientPcmSnapshot {
    /// Adds geometry once; repeated queue grants must agree with the device inventory.
    pub fn push_endpoint(&mut self, geometry: EndpointGeometry) -> bool {
        if !geometry.valid() {
            return false;
        }
        // SAFETY: this owner retains the initialized envelope throughout the call.
        unsafe {
            let Ok(current) = Self::endpoints(self.0) else {
                return false;
            };
            if let Some(existing) = current.iter().find(|entry| entry.id == geometry.id) {
                return *existing == geometry;
            }
            if current.len() == crate::control_inventory::MAX_ENTRIES {
                return false;
            }
            let entry = xpc_dictionary_create(ptr::null(), ptr::null(), 0);
            if entry.is_null() {
                return false;
            }
            xpc_dictionary_set_data(entry, c"id".as_ptr(), geometry.id.as_ptr().cast(), 16);
            xpc_dictionary_set_uint64(
                entry,
                c"render".as_ptr(),
                u64::from(geometry.render_channels),
            );
            xpc_dictionary_set_uint64(
                entry,
                c"capture".as_ptr(),
                u64::from(geometry.capture_channels),
            );
            let array = xpc_dictionary_get_value(self.0, c"endpoints".as_ptr());
            xpc_array_append_value(array, entry);
            xpc_release(entry);
        }
        true
    }
    /// Decodes the complete configured geometry, independently of client membership.
    /// # Errors
    /// Rejects invalid, duplicate or excessive endpoint configurations.
    /// # Safety
    /// `message` is a live envelope retained by the authenticated HAL caller.
    pub unsafe fn endpoints(message: Object) -> Result<Vec<EndpointGeometry>, SnapshotError> {
        // SAFETY: the caller retains message; every nested type is checked before use.
        unsafe {
            if message.is_null()
                || xpc_get_type(message) != (&raw const _xpc_type_dictionary).cast()
            {
                return Err(SnapshotError::Invalid);
            }
            let array = xpc_dictionary_get_value(message, c"endpoints".as_ptr());
            if array.is_null() || xpc_get_type(array) != (&raw const _xpc_type_array).cast() {
                return Err(SnapshotError::Invalid);
            }
            let count = xpc_array_get_count(array);
            if count > crate::control_inventory::MAX_ENTRIES {
                return Err(SnapshotError::Limit);
            }
            let mut result = Vec::new();
            result
                .try_reserve_exact(count)
                .map_err(|_| SnapshotError::Limit)?;
            for index in 0..count {
                let entry = xpc_array_get_value(array, index);
                if entry.is_null()
                    || xpc_get_type(entry) != (&raw const _xpc_type_dictionary).cast()
                {
                    return Err(SnapshotError::Invalid);
                }
                let mut bytes = 0;
                let id = xpc_dictionary_get_data(entry, c"id".as_ptr(), &raw mut bytes);
                if id.is_null() || bytes != 16 {
                    return Err(SnapshotError::Invalid);
                }
                let geometry = EndpointGeometry {
                    id: *id.cast::<[u8; 16]>(),
                    render_channels: u32::try_from(xpc_dictionary_get_uint64(
                        entry,
                        c"render".as_ptr(),
                    ))
                    .map_err(|_| SnapshotError::Invalid)?,
                    capture_channels: u32::try_from(xpc_dictionary_get_uint64(
                        entry,
                        c"capture".as_ptr(),
                    ))
                    .map_err(|_| SnapshotError::Invalid)?,
                };
                if !geometry.valid()
                    || result
                        .iter()
                        .any(|existing: &EndpointGeometry| existing.id == geometry.id)
                {
                    return Err(SnapshotError::Invalid);
                }
                result.push(geometry);
            }
            Ok(result)
        }
    }
}
