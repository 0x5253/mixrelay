//! CLAP bundle discovery and descriptor translation.

use crate::library::LoadedLibrary;
use crate::parameters::cstr_to_string;
use crate::{ClapPlugin, FORMAT};
use clap_sys::factory::plugin_factory::clap_plugin_factory;
use clap_sys::plugin::clap_plugin_descriptor;
use std::ffi::{CStr, OsStr, c_char};
use std::path::{Path, PathBuf};
use std::ptr;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::{PluginCategory, PluginInfo};
use truce_rack_core::scanner::PluginScanner;

/// CLAP scanner.
///
/// Walks the standard CLAP plugin directories for the current OS
/// and returns one entry per discovered CLAP plugin. Calling
/// [`PluginScanner::scan_path`] lets a host scan a custom
/// directory or one exact `.clap` bundle (useful for sandboxed
/// test fixtures or per-application bundled plugins).
#[derive(Debug, Default)]
pub struct ClapScanner;

impl ClapScanner {
    /// Construct a default scanner. There's no config state; the
    /// type exists so consumers have a stable handle to scan
    /// from.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PluginScanner for ClapScanner {
    type Plugin = ClapPlugin;

    fn scan(&self) -> Result<Vec<PluginInfo>> {
        let mut out = Vec::new();
        for dir in default_clap_paths() {
            scan_optional_default_dir_into(&dir, &mut out)?;
        }
        Ok(out)
    }

    fn scan_path(&self, path: &Path) -> Result<Vec<PluginInfo>> {
        let mut out = Vec::new();
        let metadata = std::fs::metadata(path)
            .map_err(|error| scan_path_io_error("inspect explicit CLAP scan path", path, &error))?;

        if has_clap_extension(path) && (metadata.is_file() || metadata.is_dir()) {
            scan_bundle_candidate_into(path, &mut out)?;
        } else if metadata.is_dir() {
            scan_dir_into(path, &mut out)?;
        } else {
            return Err(Error::PluginNotFound(format!(
                "explicit CLAP scan path must be a .clap bundle or directory: {}",
                path.display()
            )));
        }
        Ok(out)
    }

    fn load(&self, info: &PluginInfo) -> Result<Self::Plugin> {
        ClapPlugin::load_from(info)
    }
}

/// Standard locations the CLAP spec lists for each OS. Mirrors
/// what the CLAP example host walks.
#[must_use]
pub fn default_clap_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let mut user = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        user.push("Library/Audio/Plug-Ins/CLAP");
        #[cfg(target_os = "linux")]
        user.push(".clap");
        #[cfg(target_os = "windows")]
        user.push("AppData/Local/Programs/Common/CLAP");
        out.push(user);
    }
    #[cfg(target_os = "macos")]
    out.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    #[cfg(target_os = "linux")]
    out.push(PathBuf::from("/usr/lib/clap"));
    #[cfg(target_os = "windows")]
    {
        if let Some(pf) = std::env::var_os("CommonProgramFiles") {
            let mut p = PathBuf::from(pf);
            p.push("CLAP");
            out.push(p);
        }
    }
    out
}

pub(super) fn scan_optional_default_dir_into(dir: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(scan_path_io_error(
                "read default CLAP directory",
                dir,
                &error,
            ));
        }
    };
    scan_directory_entries(dir, entries, out)
}

pub(super) fn scan_dir_into(dir: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| scan_path_io_error("read explicit CLAP directory", dir, &error))?;
    scan_directory_entries(dir, entries, out)
}

pub(super) fn scan_directory_entries(
    dir: &Path,
    entries: std::fs::ReadDir,
    out: &mut Vec<PluginInfo>,
) -> Result<()> {
    for entry in entries {
        let entry = entry
            .map_err(|error| scan_path_io_error("read entry in CLAP directory", dir, &error))?;
        let path = entry.path();
        if has_clap_extension(&path) {
            scan_bundle_candidate_into(&path, out)?;
        }
    }
    Ok(())
}

pub(super) fn has_clap_extension(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("clap"))
}

pub(super) fn scan_path_io_error(action: &str, path: &Path, error: &std::io::Error) -> Error {
    Error::Other(format!("failed to {action} {}: {error}", path.display()))
}

pub(super) fn scan_bundle_candidate_into(
    bundle_path: &Path,
    out: &mut Vec<PluginInfo>,
) -> Result<()> {
    scan_bundle_into(bundle_path, out).map_err(|error| Error::LoadFailed {
        path: bundle_path.to_path_buf(),
        reason: error.to_string(),
    })
}

pub(super) fn scan_bundle_into(bundle_path: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let binary = bundle_binary_path(bundle_path);
    let handle = unsafe { LoadedLibrary::open(&binary)? };
    let entry = handle.entry()?;
    unsafe { entry.init(&binary)? };
    let factory = unsafe { entry.factory() };
    if !factory.is_null() {
        let count = unsafe { ((*factory).get_plugin_count.unwrap_or(empty_count))(factory) };
        for idx in 0..count {
            let desc =
                unsafe { ((*factory).get_plugin_descriptor.unwrap_or(empty_desc))(factory, idx) };
            if desc.is_null() {
                continue;
            }
            out.push(unsafe { descriptor_to_info(bundle_path, &*desc) });
        }
    }
    unsafe { entry.deinit() };
    Ok(())
}

pub(super) unsafe extern "C" fn empty_count(_: *const clap_plugin_factory) -> u32 {
    0
}

pub(super) unsafe extern "C" fn empty_desc(
    _: *const clap_plugin_factory,
    _: u32,
) -> *const clap_plugin_descriptor {
    ptr::null()
}

/// Per-platform `.clap` bundle layout. macOS uses NSBundle-style
/// `Contents/MacOS/<stem>`; Linux / Windows treat the `.clap`
/// file itself as the dylib.
pub(super) fn bundle_binary_path(bundle: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let stem = bundle.file_stem().unwrap_or_default();
        if bundle.is_dir() {
            return bundle.join("Contents/MacOS").join(stem);
        }
    }
    bundle.to_path_buf()
}

pub(super) unsafe fn descriptor_to_info(
    bundle_path: &Path,
    desc: &clap_plugin_descriptor,
) -> PluginInfo {
    let id = unsafe { cstr_to_string(desc.id) };
    let name = unsafe { cstr_to_string(desc.name) };
    let vendor = unsafe { cstr_to_string(desc.vendor) };
    let version_str = unsafe { cstr_to_string(desc.version) };
    let version = parse_version(&version_str);
    let (category, accepts_midi) = unsafe { categorize(desc.features) };
    PluginInfo {
        name,
        vendor,
        version,
        category,
        path: bundle_path.to_path_buf(),
        unique_id: id,
        format: FORMAT,
        has_editor: false, // honest default — set during load via the GUI ext.
        accepts_midi,
    }
}

/// CLAP feature strings live in a NULL-terminated array of C
/// strings. We scan for instrument / note-effect / analyzer
/// markers and the `note-input` / `note-effect` markers used to
/// flag MIDI capability.
pub(super) unsafe fn categorize(features: *const *const c_char) -> (PluginCategory, bool) {
    if features.is_null() {
        return (PluginCategory::Effect, false);
    }
    let mut category = PluginCategory::Effect;
    let mut accepts_midi = false;
    let mut idx = 0usize;
    loop {
        let p = unsafe { *features.add(idx) };
        if p.is_null() {
            break;
        }
        let bytes = unsafe { CStr::from_ptr(p).to_bytes() };
        match bytes {
            b"instrument" => category = PluginCategory::Instrument,
            b"note-effect" => {
                category = PluginCategory::NoteEffect;
                accepts_midi = true;
            }
            b"analyzer" => category = PluginCategory::Analyzer,
            b"utility" => category = PluginCategory::Tool,
            b"note-input" => accepts_midi = true,
            _ => {}
        }
        idx += 1;
    }
    (category, accepts_midi)
}

/// CLAP versions are `"major.minor.patch"`. We pack the first
/// three numeric components into a `u32` as
/// `(major << 16) | (minor << 8) | patch`, matching what the
/// legacy rack used.
pub(super) fn parse_version(s: &str) -> u32 {
    let mut parts = s.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    (major << 16) | (minor << 8) | patch
}
