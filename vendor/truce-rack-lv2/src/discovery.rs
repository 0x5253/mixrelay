//! LV2 catalog discovery and ownership of Lilv metadata worlds.

use crate::editor::NATIVE_UI_CLASS_URI;
use crate::{FORMAT, Lv2Plugin};
use std::{
    ffi::{CStr, CString, c_char},
    path::{Path, PathBuf},
};
use truce_rack_core::{
    error::{Error, Result},
    info::{PluginCategory, PluginInfo},
    scanner::PluginScanner,
};

/// LV2 scanner.
#[derive(Debug, Default)]
pub struct Lv2Scanner;

impl Lv2Scanner {
    /// Construct a default scanner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PluginScanner for Lv2Scanner {
    type Plugin = Lv2Plugin;

    fn scan(&self) -> Result<Vec<PluginInfo>> {
        let world = unsafe { World::new() }
            .ok_or_else(|| Error::Other("lilv_world_new returned NULL".into()))?;
        unsafe { lilv_sys::lilv_world_load_all(world.ptr) };
        Ok(unsafe { world.collect_plugin_infos() })
    }

    /// Discovers URI-identified plugins in exactly one `.lv2` bundle.
    ///
    /// Returns an error for an inaccessible bundle, missing manifest, or native world failure.
    fn scan_path(&self, path: &Path) -> Result<Vec<PluginInfo>> {
        let path = exact_bundle_path(path)?;
        // SAFETY: this new world is used only by this synchronous scan and retained until all
        // borrowed metadata has been copied into owned PluginInfo values.
        let world = unsafe { World::new() }
            .ok_or_else(|| Error::Other("lilv_world_new returned NULL".into()))?;
        world.load_bundle(&path)?;
        // SAFETY: the live world owns all metadata read during this call.
        let mut infos = unsafe { world.collect_plugin_infos() };
        for info in &mut infos {
            info.path.clone_from(&path);
        }
        Ok(infos)
    }

    fn load(&self, info: &PluginInfo) -> Result<Self::Plugin> {
        Lv2Plugin::load_from(info)
    }
}

/// RAII wrapper around `*mut LilvWorld`. Holds the world for the
/// lifetime of either a scan or a loaded plugin (instance pointers
/// reference world-owned data, so the world must outlive them).
pub(super) struct World {
    pub(super) ptr: *mut lilv_sys::LilvWorld,
}

impl World {
    unsafe fn new() -> Option<Self> {
        let ptr = unsafe { lilv_sys::lilv_world_new() };
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr })
        }
    }

    fn load_bundle(&self, path: &Path) -> Result<()> {
        let path = path_to_cstring_with_trailing_sep(path)?;
        // SAFETY: self retains the live world; path remains NUL-terminated through URI creation.
        // The temporary URI is freed after loading, before querying the world's owned metadata.
        unsafe {
            let uri = lilv_sys::lilv_new_file_uri(self.ptr, std::ptr::null(), path.as_ptr());
            if uri.is_null() {
                return Err(Error::Other("LV2 bundle URI creation failed".into()));
            }
            lilv_sys::lilv_world_load_bundle(self.ptr, uri);
            lilv_sys::lilv_node_free(uri);
            lilv_sys::lilv_world_load_specifications(self.ptr);
            lilv_sys::lilv_world_load_plugin_classes(self.ptr);
        }
        Ok(())
    }

    /// Resolve one requested plugin while retaining the world that owns its native pointer.
    pub(super) fn load_plugin(
        info: &PluginInfo,
    ) -> Result<(Box<Self>, *const lilv_sys::LilvPlugin)> {
        // SAFETY: the new native world is immediately boxed and retained with its selected
        // plugin pointer until the owning Lv2Plugin has freed its native instance.
        let world = Box::new(unsafe { Self::new() }.ok_or_else(|| Error::LoadFailed {
            path: info.path.clone(),
            reason: "lilv_world_new returned NULL".into(),
        })?);
        if info.path.as_os_str().is_empty() {
            // SAFETY: the returned box retains this world for every borrowed native object.
            unsafe { lilv_sys::lilv_world_load_all(world.ptr) };
        } else {
            world.load_bundle(&exact_bundle_path(&info.path)?)?;
        }

        let uri = CString::new(info.unique_id.clone()).map_err(|_| Error::LoadFailed {
            path: info.path.clone(),
            reason: format!("lv2 unique_id contains NUL: {:?}", info.unique_id),
        })?;
        // SAFETY: the world and NUL-terminated URI remain live through lookup; this node is freed.
        let uri_node = unsafe { lilv_sys::lilv_new_uri(world.ptr, uri.as_ptr()) };
        if uri_node.is_null() {
            return Err(Error::LoadFailed {
                path: info.path.clone(),
                reason: format!("lilv_new_uri failed for {:?}", info.unique_id),
            });
        }
        // SAFETY: Lilv returns world-owned plugin metadata. The returned plugin pointer is
        // retained only alongside the boxed world and its temporary URI node is freed below.
        let plugins = unsafe { lilv_sys::lilv_world_get_all_plugins(world.ptr) };
        let plugin = unsafe { lilv_sys::lilv_plugins_get_by_uri(plugins, uri_node) };
        unsafe { lilv_sys::lilv_node_free(uri_node) };
        if plugin.is_null() {
            return Err(Error::LoadFailed {
                path: info.path.clone(),
                reason: format!("no LV2 plugin matching URI {:?}", info.unique_id),
            });
        }
        Ok((world, plugin))
    }

    unsafe fn collect_plugin_infos(&self) -> Vec<PluginInfo> {
        let mut out = Vec::new();
        let plugins = unsafe { lilv_sys::lilv_world_get_all_plugins(self.ptr) };
        if plugins.is_null() {
            return out;
        }
        // SAFETY: this world remains live until the scan and its temporary URI nodes finish.
        let uris = unsafe { CatalogUris::new(self) };
        let mut it = unsafe { lilv_sys::lilv_plugins_begin(plugins) };
        loop {
            if unsafe { lilv_sys::lilv_plugins_is_end(plugins, it) } {
                break;
            }
            let plugin = unsafe { lilv_sys::lilv_plugins_get(plugins, it) };
            if !plugin.is_null() {
                out.push(unsafe { plugin_to_info(self.ptr, plugin, &uris) });
            }
            it = unsafe { lilv_sys::lilv_plugins_next(plugins, it) };
        }
        out
    }

    unsafe fn new_uri(&self, uri: *const c_char) -> *mut lilv_sys::LilvNode {
        unsafe { lilv_sys::lilv_new_uri(self.ptr, uri.cast()) }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        unsafe { lilv_sys::lilv_world_free(self.ptr) };
    }
}

// SAFETY: lilv's world is meant to be single-threaded for mutation
// but we hand the wrapper between activate/process on the same
// thread the host owns. We never share it.
unsafe impl Send for World {}

/// Temporary class queries share the same world as the plugin metadata being scanned.
struct CatalogUris {
    audio: *mut lilv_sys::LilvNode,
    input: *mut lilv_sys::LilvNode,
    output: *mut lilv_sys::LilvNode,
    atom: *mut lilv_sys::LilvNode,
}

impl CatalogUris {
    /// # Safety
    /// `world` must remain alive until this temporary URI owner is dropped.
    unsafe fn new(world: &World) -> Self {
        // SAFETY: each URI is static and NUL-terminated, and the caller retains this world.
        unsafe {
            Self {
                audio: world.new_uri(lilv_sys::LILV_URI_AUDIO_PORT.as_ptr().cast()),
                input: world.new_uri(lilv_sys::LILV_URI_INPUT_PORT.as_ptr().cast()),
                output: world.new_uri(lilv_sys::LILV_URI_OUTPUT_PORT.as_ptr().cast()),
                atom: world.new_uri(lilv_sys::LILV_URI_ATOM_PORT.as_ptr().cast()),
            }
        }
    }
}

impl Drop for CatalogUris {
    fn drop(&mut self) {
        // SAFETY: this owner frees each temporary native node exactly once before World drops.
        unsafe {
            lilv_sys::lilv_node_free(self.audio);
            lilv_sys::lilv_node_free(self.input);
            lilv_sys::lilv_node_free(self.output);
            lilv_sys::lilv_node_free(self.atom);
        }
    }
}

unsafe fn plugin_to_info(
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
    uris: &CatalogUris,
) -> PluginInfo {
    let uri_node = unsafe { lilv_sys::lilv_plugin_get_uri(plugin) };
    let uri = unsafe { node_to_uri_string(uri_node) };

    let name_node = unsafe { lilv_sys::lilv_plugin_get_name(plugin) };
    let name = unsafe { node_to_string_owned(name_node) };
    unsafe { lilv_sys::lilv_node_free(name_node) };

    let author_node = unsafe { lilv_sys::lilv_plugin_get_author_name(plugin) };
    let vendor = if author_node.is_null() {
        String::new()
    } else {
        let v = unsafe { node_to_string_owned(author_node) };
        unsafe { lilv_sys::lilv_node_free(author_node) };
        v
    };

    let mut audio_in = 0u32;
    let mut audio_out = 0u32;
    let mut accepts_midi = false;
    let count = unsafe { lilv_sys::lilv_plugin_get_num_ports(plugin) };
    for idx in 0..count {
        let port = unsafe { lilv_sys::lilv_plugin_get_port_by_index(plugin, idx) };
        if port.is_null() {
            continue;
        }
        let is_input = unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.input) };
        if unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.audio) } {
            if is_input {
                audio_in += 1;
            } else if unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.output) } {
                audio_out += 1;
            }
        } else if is_input && unsafe { lilv_sys::lilv_port_is_a(plugin, port, uris.atom) } {
            // Best-effort: any atom-port input is treated as a MIDI
            // sink for the catalog. The actual MIDI-vs-not check
            // requires walking the port's `atom:supports` triples,
            // which we do at load time.
            accepts_midi = true;
        }
    }
    let category = if audio_in == 0 && audio_out > 0 {
        PluginCategory::Instrument
    } else {
        PluginCategory::Effect
    };

    let has_editor = unsafe { has_native_ui(world, plugin) };

    PluginInfo {
        name,
        vendor,
        version: 0,
        category,
        path: std::path::PathBuf::new(),
        unique_id: uri,
        format: FORMAT,
        has_editor,
        accepts_midi,
    }
}

/// True if the plugin advertises a UI of this platform's native class
/// (`CocoaUI` / `WindowsUI` / `X11UI`).
unsafe fn has_native_ui(
    world_ptr: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
) -> bool {
    let uis = unsafe { lilv_sys::lilv_plugin_get_uis(plugin) };
    if uis.is_null() {
        return false;
    }
    if world_ptr.is_null() {
        unsafe { lilv_sys::lilv_uis_free(uis) };
        return false;
    }
    let class_node =
        unsafe { lilv_sys::lilv_new_uri(world_ptr, NATIVE_UI_CLASS_URI.as_ptr().cast()) };
    let mut found = false;
    let mut it = unsafe { lilv_sys::lilv_uis_begin(uis) };
    while !unsafe { lilv_sys::lilv_uis_is_end(uis, it) } {
        let ui = unsafe { lilv_sys::lilv_uis_get(uis, it) };
        if !ui.is_null() && unsafe { lilv_sys::lilv_ui_is_a(ui, class_node) } {
            found = true;
            break;
        }
        it = unsafe { lilv_sys::lilv_uis_next(uis, it) };
    }
    unsafe {
        lilv_sys::lilv_node_free(class_node);
        lilv_sys::lilv_uis_free(uis);
    }
    found
}

pub(super) unsafe fn node_to_uri_string(node: *const lilv_sys::LilvNode) -> String {
    if node.is_null() {
        return String::new();
    }
    let p = unsafe { lilv_sys::lilv_node_as_uri(node) };
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe fn node_to_string_owned(node: *mut lilv_sys::LilvNode) -> String {
    if node.is_null() {
        return String::new();
    }
    let p = unsafe { lilv_sys::lilv_node_as_string(node) };
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

pub(super) fn path_to_cstring_with_trailing_sep(p: &Path) -> Result<CString> {
    let mut s = p
        .to_str()
        .ok_or_else(|| Error::Other("LV2 bundle path is not UTF-8".into()))?
        .to_owned();
    if !s.ends_with(std::path::MAIN_SEPARATOR) {
        s.push(std::path::MAIN_SEPARATOR);
    }
    CString::new(s).map_err(|_| Error::Other("LV2 bundle path contains NUL".into()))
}

fn exact_bundle_path(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.is_dir()
        || path.extension().and_then(|extension| extension.to_str()) != Some("lv2")
        || !path.join("manifest.ttl").is_file()
    {
        return Err(Error::Other(
            "LV2 path must be one .lv2 bundle containing manifest.ttl".into(),
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests;
