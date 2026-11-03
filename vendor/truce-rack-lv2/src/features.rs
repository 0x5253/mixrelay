//! Instance-owned, heap-stable LV2 host features and URI mapping.

use crate::options;
use lv2_raw::{
    core::LV2Feature,
    urid::{LV2Urid, LV2UridMap, LV2UridMapHandle},
};
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    sync::Mutex,
};

pub(super) const LV2_URID_MAP_URI: &[u8] = b"http://lv2plug.in/ns/ext/urid#map\0";
const LV2_BOUNDED_BLOCK_URI: &[u8] = b"http://lv2plug.in/ns/ext/buf-size#boundedBlockLength\0";
/// Bidirectional URI ↔ u32 map handed to LV2 plugins as the
/// standard `urid#map` feature.
pub(super) struct UridMap {
    entries: Mutex<HashMap<CString, LV2Urid>>,
}

impl UridMap {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn map_uri(&self, uri: &CStr) -> LV2Urid {
        // Mapping is instance-local and serialized by the host feature contract. Poisoned
        // control-thread work cannot turn a later FFI lookup into a Rust panic.
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&id) = entries.get(uri) {
            return id;
        }
        let Some(id) = entries
            .len()
            .checked_add(1)
            .and_then(|id| LV2Urid::try_from(id).ok())
        else {
            return 0;
        };
        entries.insert(uri.to_owned(), id);
        id
    }

    pub(super) fn uri(&self, id: LV2Urid) -> Option<String> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find_map(|(uri, value)| {
                (*value == id)
                    .then(|| uri.to_str().ok().map(str::to_owned))
                    .flatten()
            })
    }
}

pub(super) extern "C" fn map_callback(handle: LV2UridMapHandle, uri: *const c_char) -> LV2Urid {
    if handle.is_null() || uri.is_null() {
        return 0;
    }
    // SAFETY: `handle` is the `&UridMap` we set into the feature
    // struct at instantiate time; lilv passes it back unchanged.
    let map = unsafe { &*(handle as *const UridMap) };
    // SAFETY: LV2 map callers provide a NUL-terminated URI for the duration of this callback.
    let cstr = unsafe { CStr::from_ptr(uri) };
    map.map_uri(cstr)
}

/// Self-contained LV2 feature list. Heap-allocated so the pointers
/// fed to `lilv_plugin_instantiate` stay valid for the instance's
/// lifetime even if `Lv2Plugin` is moved.
pub(super) struct Features {
    pub(super) urid_map: UridMap,
    pub(super) options: Box<options::HostOptions>,
    // `LV2UridMap.handle` points at `urid_map` — the box's pinned
    // address. `LV2UridMap.map` is the static callback. Held inline
    // so its address can be taken for `feature.data`.
    map_struct: LV2UridMap,
    // The features array proper. Each `data` field points at a
    // sibling field of the same `Features` allocation.
    feature_storage: Vec<LV2Feature>,
    // Null-terminated pointer array — what `lilv_plugin_instantiate`
    // actually consumes.
    pub(super) feature_ptrs: Vec<*const LV2Feature>,
}

impl Features {
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "Native feature data and URID handles retain pointers into this allocation"
    )]
    pub(super) fn new() -> Box<Self> {
        // Build in two passes so the `Box`'s heap address is stable
        // before we record interior pointers.
        let urid_map = UridMap::new();
        let options = options::HostOptions::new(&urid_map);
        let mut boxed = Box::new(Features {
            urid_map,
            options,
            map_struct: LV2UridMap {
                handle: std::ptr::null_mut(),
                map: map_callback,
            },
            feature_storage: Vec::with_capacity(3),
            feature_ptrs: Vec::with_capacity(4),
        });
        let urid_map_handle: *mut UridMap = &mut boxed.urid_map as *mut UridMap;
        boxed.map_struct.handle = urid_map_handle.cast::<c_void>();
        boxed.feature_storage.push(LV2Feature {
            uri: LV2_URID_MAP_URI.as_ptr().cast::<c_char>(),
            data: (&boxed.map_struct) as *const LV2UridMap as *mut c_void,
        });
        boxed.feature_storage.push(LV2Feature {
            uri: options::FEATURE_URI.as_ptr().cast(),
            data: boxed.options.data(),
        });
        boxed.feature_storage.push(LV2Feature {
            uri: LV2_BOUNDED_BLOCK_URI.as_ptr().cast(),
            data: std::ptr::null_mut(),
        });
        for feature in &boxed.feature_storage {
            boxed.feature_ptrs.push(feature);
        }
        boxed.feature_ptrs.push(std::ptr::null());
        boxed
    }
}

#[cfg(test)]
#[path = "features/tests.rs"]
mod tests;
