//! `MixRelay` patch: native LV2 State interface plus input control-port values.
//!
//! State calls run on the worker control thread with DSP/editor access serialized. Lilv's
//! convenience save API discards opaque properties after a native error, and its restore API
//! returns void; call the standard LV2 interface directly so failures cannot become successful
//! parameter-only snapshots. See <https://lv2plug.in/ns/ext/state>. Host-private regular-file
//! assets referenced through `atom:Path` are packaged with the snapshot; nested URID containers
//! and paths outside host-owned state directories remain unsupported.

use crate::{Lv2Plugin, features::UridMap, ports::PortKind};
use assets::{FreePath, MakePath, MapPath};
use lv2_raw::core::LV2Feature;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, c_void},
};
use truce_rack_core::error::{Error, Result};

mod assets;

// Matches the host StateStore's single-blob bound, without a vendor -> host dependency.
const LIMIT: usize = 64 * 1024 * 1024;
const PROPERTY_LIMIT: usize = 1024 * 1024;
const COUNT_LIMIT: usize = 4096;
const PORTABLE_POD: u32 = 3;
const STATE_INTERFACE: &[u8] = b"http://lv2plug.in/ns/ext/state#interface\0";
const ATOM: &str = "http://lv2plug.in/ns/ext/atom#";

type Store = unsafe extern "C" fn(*mut c_void, u32, *const c_void, usize, u32, u32) -> u32;
type Extension = extern "C" fn(*const u8) -> *const c_void;
type Retrieve =
    unsafe extern "C" fn(*mut c_void, u32, *mut usize, *mut u32, *mut u32) -> *const c_void;
#[repr(C)]
struct StateInterface {
    save: Option<
        unsafe extern "C" fn(*mut c_void, Store, *mut c_void, u32, *const *const LV2Feature) -> u32,
    >,
    restore: Option<
        unsafe extern "C" fn(
            *mut c_void,
            Retrieve,
            *mut c_void,
            u32,
            *const *const LV2Feature,
        ) -> u32,
    >,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Snapshot {
    version: u32,
    plugin_uri: String,
    byte_order: String,
    native_state: bool,
    ports: BTreeMap<String, f32>,
    properties: Vec<Property>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    assets: Vec<Asset>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    path: String,
    sha256: String,
    data: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Property {
    key: String,
    value_type: String,
    value: PropertyValue,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
enum PropertyValue {
    Bytes(Vec<u8>),
    Urid(String),
    Path(String),
}

fn error(message: impl Into<String>) -> Error {
    Error::Other(message.into())
}
fn byte_order() -> &'static str {
    if cfg!(target_endian = "little") {
        "little"
    } else {
        "big"
    }
}

fn uri(map: &UridMap, id: u32) -> Option<String> {
    map.uri(id)
}

fn map_uri(map: &UridMap, value: &str) -> Result<u32> {
    if value.is_empty() || value.len() > 4096 || !value.contains(':') {
        return Err(error("LV2 state contains an invalid URI"));
    }
    let value = std::ffi::CString::new(value).map_err(|_| error("LV2 state URI contains NUL"))?;
    Ok(map.map_uri(&value))
}

fn validate_type(value: &str) -> Result<()> {
    if matches!(
        value.strip_prefix(ATOM),
        Some(
            "Object"
                | "Blank"
                | "Resource"
                | "Literal"
                | "Property"
                | "Tuple"
                | "Vector"
                | "Sequence"
                | "Event"
        )
    ) {
        return Err(error(format!(
            "LV2 state type requires unsupported asset/container packaging: {value}"
        )));
    }
    Ok(())
}

fn interface(plugin: &Lv2Plugin) -> Result<Option<&StateInterface>> {
    if plugin.instance.is_null() {
        return Err(error("LV2 instance is not active"));
    }
    // SAFETY: the plugin owns this live instance/descriptor throughout the serialized call.
    // lv2_raw represents the nullable extension_data C callback as non-null fn; read its field
    // through an Option function pointer to respect the ABI rather than calling a null address.
    unsafe {
        let descriptor = (*plugin.instance).lv2_descriptor;
        if descriptor.is_null() {
            return Err(error("LV2 descriptor is unavailable"));
        }
        let extension = std::ptr::addr_of!((*descriptor).extension_data)
            .cast::<Option<Extension>>()
            .read();
        Ok(extension.and_then(|get| {
            get(STATE_INTERFACE.as_ptr())
                .cast::<StateInterface>()
                .as_ref()
        }))
    }
}

struct StoreContext<'a> {
    map: &'a UridMap,
    properties: Vec<Property>,
    bytes: usize,
    failure: Option<String>,
}

unsafe extern "C" fn store(
    handle: *mut c_void,
    key: u32,
    value: *const c_void,
    size: usize,
    value_type: u32,
    flags: u32,
) -> u32 {
    // SAFETY: our synchronous native save call owns this context for the whole callback window.
    let context = unsafe { &mut *handle.cast::<StoreContext<'_>>() };
    let result = (|| {
        if flags & PORTABLE_POD != PORTABLE_POD
            || value.is_null()
            || size == 0
            || size > PROPERTY_LIMIT
            || context.bytes.saturating_add(size) > PROPERTY_LIMIT
            || context.properties.len() >= COUNT_LIMIT
        {
            return Err(error(
                "LV2 state property is nonportable, invalid, or exceeds the capture limit",
            ));
        }
        let key = uri(context.map, key).ok_or_else(|| error("LV2 state key is not mapped"))?;
        let value_type =
            uri(context.map, value_type).ok_or_else(|| error("LV2 state type is not mapped"))?;
        validate_type(&value_type)?;
        if context
            .properties
            .iter()
            .any(|property| property.key == key)
        {
            return Err(error("LV2 state key was stored twice"));
        }
        // SAFETY: the LV2 save contract guarantees a readable size-byte property until return;
        // the bound above limits every allocation and we copy before the callback returns.
        let bytes = unsafe { std::slice::from_raw_parts(value.cast::<u8>(), size) };
        let value = if value_type == format!("{ATOM}Path") {
            let path = CStr::from_bytes_with_nul(bytes)
                .map_err(|_| error("LV2 atom:Path is not NUL-terminated"))?
                .to_str()
                .map_err(|_| error("LV2 atom:Path is not UTF-8"))?;
            PropertyValue::Path(path.to_owned())
        } else if value_type == format!("{ATOM}URID") {
            let raw: [u8; 4] = bytes
                .try_into()
                .map_err(|_| error("LV2 atom:URID has invalid size"))?;
            PropertyValue::Urid(
                uri(context.map, u32::from_ne_bytes(raw))
                    .ok_or_else(|| error("LV2 state value URID is not mapped"))?,
            )
        } else {
            PropertyValue::Bytes(bytes.to_vec())
        };
        context.bytes += size;
        context.properties.push(Property {
            key,
            value_type,
            value,
        });
        Ok(())
    })();
    if let Err(failure) = result {
        context.failure = Some(failure.to_string());
        1
    } else {
        0
    }
}

fn input_ports(plugin: &Lv2Plugin) -> Result<BTreeMap<String, usize>> {
    let mut result = BTreeMap::new();
    for (index, port) in plugin.bindings.ports.iter().enumerate() {
        if !port.is_input || !matches!(port.kind, PortKind::Control { .. }) {
            continue;
        }
        // SAFETY: all nodes are borrowed from this plugin's retained Lilv world.
        let symbol = unsafe {
            let native = lilv_sys::lilv_plugin_get_port_by_index(plugin.plugin, port.index);
            if native.is_null() {
                return Err(error("LV2 control port is unavailable"));
            }
            let node = lilv_sys::lilv_port_get_symbol(plugin.plugin, native);
            if node.is_null() {
                return Err(error("LV2 control symbol is unavailable"));
            }
            let raw = lilv_sys::lilv_node_as_string(node);
            if raw.is_null() {
                return Err(error("LV2 control symbol is unavailable"));
            }
            CStr::from_ptr(raw)
                .to_str()
                .map_err(|_| error("LV2 control symbol is not UTF-8"))?
                .to_owned()
        };
        let offset = plugin.bindings.control_value_offset[index];
        if offset >= plugin.bindings.control_values.len() || result.insert(symbol, offset).is_some()
        {
            return Err(error("LV2 control port layout is invalid"));
        }
    }
    Ok(result)
}

fn capture_native_assets(
    plugin: &Lv2Plugin,
    interface: &StateInterface,
    context: &mut StoreContext<'_>,
) -> Result<Vec<Asset>> {
    if interface.restore.is_none() {
        return Err(error("LV2 native state restore is unavailable"));
    }
    let mut asset_guard = plugin
        .asset_dir
        .lock()
        .map_err(|_| error("LV2 asset directory lock is poisoned"))?;
    if asset_guard.is_none() {
        *asset_guard = Some(
            tempfile::Builder::new()
                .prefix("mixrelay-lv2-assets-")
                .tempdir()
                .map_err(|e| error(e.to_string()))?,
        );
    }
    let root = asset_guard.as_ref().expect("created above").path();
    let mut paths = assets::SavePaths::new(
        root,
        plugin.restored_assets.as_ref().map(tempfile::TempDir::path),
    );
    invoke_native_save(plugin, interface, context, &mut paths)?;
    paths.package()
}

fn invoke_native_save(
    plugin: &Lv2Plugin,
    interface: &StateInterface,
    context: &mut StoreContext<'_>,
    paths: &mut assets::SavePaths,
) -> Result<()> {
    let save = interface
        .save
        .ok_or_else(|| error("LV2 native state save is unavailable"))?;
    let handle = (paths as *mut assets::SavePaths).cast();
    let map_path = MapPath {
        handle,
        abstract_path: assets::save_abstract,
        absolute_path: assets::save_absolute,
    };
    let make_path = MakePath {
        handle,
        path: assets::save_make,
    };
    let free_path = FreePath {
        handle: std::ptr::null_mut(),
        free_path: assets::free_path,
    };
    let path_features = [
        LV2Feature {
            uri: assets::MAP_PATH_URI.as_ptr().cast(),
            data: (&map_path as *const MapPath).cast_mut().cast(),
        },
        LV2Feature {
            uri: assets::MAKE_PATH_URI.as_ptr().cast(),
            data: (&make_path as *const MakePath).cast_mut().cast(),
        },
        LV2Feature {
            uri: assets::FREE_PATH_URI.as_ptr().cast(),
            data: (&free_path as *const FreePath).cast_mut().cast(),
        },
    ];
    let mut features =
        plugin.features.feature_ptrs[..plugin.features.feature_ptrs.len() - 1].to_vec();
    features.extend(path_features.iter().map(std::ptr::from_ref));
    features.push(std::ptr::null());
    // SAFETY: caller serializes DSP/editor mutation; stack features and callback contexts remain
    // alive through this synchronous LV2 call, and returned strings are malloc-owned.
    let status = unsafe {
        save(
            (*plugin.instance).lv2_handle,
            store,
            (context as *mut StoreContext<'_>).cast(),
            PORTABLE_POD,
            features.as_ptr(),
        )
    };
    if let Some(failure) = paths.failure.take() {
        return Err(error(failure));
    }
    if let Some(failure) = context.failure.take() {
        return Err(error(failure));
    }
    if status != 0 {
        return Err(error(format!(
            "LV2 native state save failed with status {status}"
        )));
    }
    if context
        .properties
        .iter()
        .any(|property| matches!(&property.value, PropertyValue::Path(path) if !paths.has_abstract_path(path)))
    {
        return Err(error("LV2 atom:Path was not mapped by state:mapPath"));
    }
    Ok(())
}

pub(super) fn save(plugin: &Lv2Plugin) -> Result<Vec<u8>> {
    let interface = interface(plugin)?;
    let mut context = StoreContext {
        map: &plugin.features.urid_map,
        properties: Vec::new(),
        bytes: 0,
        failure: None,
    };
    let packaged_assets = match interface {
        Some(interface) => capture_native_assets(plugin, interface, &mut context)?,
        None => Vec::new(),
    };
    let ports: BTreeMap<_, _> = input_ports(plugin)?
        .into_iter()
        .map(|(symbol, offset)| (symbol, plugin.effective_control_value(offset)))
        .collect();
    if ports.len() > COUNT_LIMIT || ports.values().any(|value| !value.is_finite()) {
        return Err(error(
            "LV2 input control state is invalid or exceeds its count limit",
        ));
    }
    let snapshot = Snapshot {
        version: 1,
        plugin_uri: plugin.info.unique_id.clone(),
        byte_order: byte_order().into(),
        native_state: interface.is_some(),
        ports,
        properties: context.properties,
        assets: packaged_assets,
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|failure| error(failure.to_string()))?;
    if bytes.len() > LIMIT {
        return Err(error("LV2 state exceeds 64 MiB"));
    }
    Ok(bytes)
}

struct RestoreProperty {
    key: u32,
    value_type: u32,
    bytes: Vec<u8>,
}
unsafe extern "C" fn retrieve(
    handle: *mut c_void,
    key: u32,
    size: *mut usize,
    value_type: *mut u32,
    flags: *mut u32,
) -> *const c_void {
    // SAFETY: the immutable vector and every owned value live throughout native restore.
    let properties = unsafe { &*handle.cast::<Vec<RestoreProperty>>() };
    let Some(property) = properties.iter().find(|property| property.key == key) else {
        return std::ptr::null();
    };
    unsafe {
        if !size.is_null() {
            *size = property.bytes.len();
        }
        if !value_type.is_null() {
            *value_type = property.value_type;
        }
        if !flags.is_null() {
            *flags = PORTABLE_POD;
        }
    }
    property.bytes.as_ptr().cast()
}

fn decode(
    bytes: &[u8],
    plugin_uri: &str,
    map: &UridMap,
) -> Result<(Snapshot, Vec<RestoreProperty>)> {
    if bytes.len() > LIMIT {
        return Err(error("LV2 state exceeds 64 MiB"));
    }
    let snapshot: Snapshot = serde_json::from_slice(bytes)
        .map_err(|failure| error(format!("invalid LV2 state: {failure}")))?;
    if snapshot.version != 1
        || snapshot.plugin_uri != plugin_uri
        || snapshot.byte_order != byte_order()
        || snapshot.properties.len() > COUNT_LIMIT
        || snapshot.assets.len() > assets::MAX_ASSETS
        || snapshot.ports.len() > COUNT_LIMIT
        || (!snapshot.native_state
            && (!snapshot.properties.is_empty() || !snapshot.assets.is_empty()))
    {
        return Err(error(
            "incompatible LV2 state identity, version, byte order, or property count",
        ));
    }
    let mut keys = BTreeSet::new();
    let mut total = 0_usize;
    let mut properties = Vec::new();
    for property in &snapshot.properties {
        validate_type(&property.value_type)?;
        if !keys.insert(&property.key) {
            return Err(error("duplicate LV2 state key"));
        }
        let bytes = match &property.value {
            PropertyValue::Urid(uri) if property.value_type == format!("{ATOM}URID") => {
                map_uri(map, uri)?.to_ne_bytes().to_vec()
            }
            PropertyValue::Path(path) if property.value_type == format!("{ATOM}Path") => {
                if !snapshot.assets.iter().any(|asset| &asset.path == path) {
                    return Err(error("LV2 state references a missing asset"));
                }
                std::ffi::CString::new(path.as_str())
                    .map_err(|_| error("LV2 asset path contains NUL"))?
                    .into_bytes_with_nul()
            }
            PropertyValue::Bytes(bytes)
                if property.value_type != format!("{ATOM}URID")
                    && property.value_type != format!("{ATOM}Path") =>
            {
                bytes.clone()
            }
            _ => return Err(error("LV2 state value does not match its type")),
        };
        total = total.saturating_add(bytes.len());
        if bytes.is_empty() || total > PROPERTY_LIMIT {
            return Err(error("LV2 state property size exceeds limit"));
        }
        properties.push(RestoreProperty {
            key: map_uri(map, &property.key)?,
            value_type: map_uri(map, &property.value_type)?,
            bytes,
        });
    }
    if snapshot.ports.values().any(|value| !value.is_finite()) {
        return Err(error("LV2 control state contains nonfinite values"));
    }
    Ok((snapshot, properties))
}

pub(super) fn restore(plugin: &mut Lv2Plugin, bytes: &[u8]) -> Result<()> {
    let (snapshot, properties) = decode(bytes, &plugin.info.unique_id, &plugin.features.urid_map)?;
    let ports = input_ports(plugin)?;
    if ports.keys().ne(snapshot.ports.keys()) {
        return Err(error("LV2 input control symbols changed"));
    }
    let interface = interface(plugin)?;
    if snapshot.native_state != interface.is_some() {
        return Err(error("LV2 native state capability changed"));
    }
    let restored_assets = if snapshot.assets.is_empty() {
        None
    } else {
        Some(assets::materialize(&snapshot.assets)?)
    };
    // All payload and control symbols are validated before invoking the native restore method.
    // Native restore can mutate before returning failure: the caller restores only a newly
    // prepared worker and discards that worker on failure, never commits it into a live graph.
    if let Some(interface) = interface {
        let restore = interface
            .restore
            .ok_or_else(|| error("LV2 native state restore is unavailable"))?;
        let mut paths = assets::RestorePaths::new(
            restored_assets
                .as_ref()
                .map_or_else(|| std::path::Path::new(""), tempfile::TempDir::path),
            &snapshot.assets,
        );
        let handle = (&mut paths as *mut assets::RestorePaths).cast();
        let map_path = MapPath {
            handle,
            abstract_path: assets::restore_abstract,
            absolute_path: assets::restore_absolute,
        };
        let free_path = FreePath {
            handle: std::ptr::null_mut(),
            free_path: assets::free_path,
        };
        let path_features = [
            LV2Feature {
                uri: assets::MAP_PATH_URI.as_ptr().cast(),
                data: (&map_path as *const MapPath).cast_mut().cast(),
            },
            LV2Feature {
                uri: assets::FREE_PATH_URI.as_ptr().cast(),
                data: (&free_path as *const FreePath).cast_mut().cast(),
            },
        ];
        let mut features =
            plugin.features.feature_ptrs[..plugin.features.feature_ptrs.len() - 1].to_vec();
        features.extend(path_features.iter().map(std::ptr::from_ref));
        features.push(std::ptr::null());
        // SAFETY: all asset files, features and properties outlive the synchronous native call.
        let status = unsafe {
            restore(
                (*plugin.instance).lv2_handle,
                retrieve,
                (&properties as *const Vec<RestoreProperty>)
                    .cast_mut()
                    .cast(),
                PORTABLE_POD,
                features.as_ptr(),
            )
        };
        if let Some(failure) = paths.failure {
            return Err(error(failure));
        }
        if status != 0 {
            return Err(error(format!(
                "LV2 native state restore failed with status {status}"
            )));
        }
    }
    plugin.restored_assets = restored_assets;
    for (symbol, offset) in ports {
        plugin.bindings.control_values[offset] = snapshot.ports[&symbol];
    }
    Ok(())
}

#[cfg(test)]
mod tests;
