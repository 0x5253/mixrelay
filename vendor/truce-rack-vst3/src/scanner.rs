//! Discover VST3 classes without constructing plug-in instances.

use crate::{
    FORMAT, Vst3Plugin,
    identity::{char8_array_to_string, tuid_to_hex},
    module::LoadedModule,
};
use std::{
    ffi::OsStr,
    fs::ReadDir,
    path::{Path, PathBuf},
};
use truce_rack_core::{
    error::{Error, Result},
    info::{PluginCategory, PluginInfo},
    scanner::PluginScanner,
};
use vst3::Steinberg::{
    IPluginFactory2, IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo, PClassInfo_,
    PClassInfo2, kResultOk,
};

/// VST3 scanner.
#[derive(Debug, Default)]
pub struct Vst3Scanner;

impl Vst3Scanner {
    /// Construct a default scanner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PluginScanner for Vst3Scanner {
    type Plugin = Vst3Plugin;

    fn scan(&self) -> Result<Vec<PluginInfo>> {
        let mut out = Vec::new();
        for dir in default_vst3_paths() {
            scan_optional_default_dir(&dir, &mut out)?;
        }
        Ok(out)
    }

    fn scan_path(&self, path: &Path) -> Result<Vec<PluginInfo>> {
        let mut out = Vec::new();
        if path.extension() == Some(OsStr::new("vst3")) {
            scan_bundle(path, &mut out)?;
        } else {
            scan_dir_into(path, &mut out)?;
        }
        Ok(out)
    }

    fn load(&self, info: &PluginInfo) -> Result<Self::Plugin> {
        Vst3Plugin::load_from(info)
    }
}

/// Standard VST3 install locations for the current OS.
#[must_use]
pub fn default_vst3_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let mut user = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        user.push("Library/Audio/Plug-Ins/VST3");
        #[cfg(target_os = "linux")]
        user.push(".vst3");
        #[cfg(target_os = "windows")]
        user.push("AppData/Local/Programs/Common/VST3");
        out.push(user);
    }
    #[cfg(target_os = "macos")]
    out.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    #[cfg(target_os = "linux")]
    out.push(PathBuf::from("/usr/lib/vst3"));
    #[cfg(target_os = "windows")]
    {
        if let Some(pf) = std::env::var_os("CommonProgramFiles") {
            let mut p = PathBuf::from(pf);
            p.push("VST3");
            out.push(p);
        }
    }
    out
}

fn scan_optional_default_dir(dir: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(scan_directory_error(dir, &error)),
    };
    scan_entries(dir, entries, out)
}

fn scan_dir_into(dir: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|error| scan_directory_error(dir, &error))?;
    scan_entries(dir, entries, out)
}

fn scan_entries(dir: &Path, entries: ReadDir, out: &mut Vec<PluginInfo>) -> Result<()> {
    for entry in entries {
        let entry = entry.map_err(|error| scan_directory_error(dir, &error))?;
        let path = entry.path();
        if path.extension() != Some(OsStr::new("vst3")) {
            continue;
        }
        scan_bundle(&path, out)?;
    }
    Ok(())
}

fn scan_bundle(bundle: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let mut bundle_plugins = Vec::new();
    scan_bundle_into(bundle, &mut bundle_plugins)
        .map_err(|error| scan_bundle_error(bundle, &error))?;
    out.extend(bundle_plugins);
    Ok(())
}

fn scan_directory_error(dir: &Path, error: &std::io::Error) -> Error {
    Error::Other(format!(
        "failed to read VST3 scan directory {}: {error}",
        dir.display()
    ))
}

fn scan_bundle_error(bundle: &Path, error: &Error) -> Error {
    Error::Other(format!(
        "failed to scan VST3 bundle {}: {error}",
        bundle.display()
    ))
}

fn scan_bundle_into(bundle: &Path, out: &mut Vec<PluginInfo>) -> Result<()> {
    let module = unsafe { LoadedModule::open(bundle) }?;
    let factory = module.factory()?;
    let factory2 = factory.as_com_ref().cast::<IPluginFactory2>();
    let count = unsafe { factory.countClasses() };
    if count < 0 {
        return Err(Error::Other(format!(
            "IPluginFactory::countClasses returned invalid count {count}"
        )));
    }
    let mut info = empty_pclass_info();
    for idx in 0..count {
        let status = unsafe { factory.getClassInfo(idx, &raw mut info) };
        if status != kResultOk {
            return Err(Error::Format {
                format: FORMAT,
                code: i64::from(status),
                message: format!("IPluginFactory::getClassInfo({idx}) failed"),
            });
        }
        let category = char8_array_to_string(&info.category);
        if category != "Audio Module Class" {
            continue;
        }
        let name = char8_array_to_string(&info.name);
        // Audio Module Class includes instruments. Use the extended class metadata before
        // loading any instances so an instrument is not probed as an insert effect.
        // SAFETY: PClassInfo2 contains only integers and fixed-size integer arrays.
        let mut extended: PClassInfo2 = unsafe { std::mem::zeroed() };
        let has_extended = factory2.as_ref().is_some_and(|factory| {
            (unsafe { factory.getClassInfo2(idx, &raw mut extended) }) == kResultOk
        });
        let category = if has_extended {
            class_category(&char8_array_to_string(&extended.subCategories))
        } else {
            PluginCategory::Effect
        };
        out.push(PluginInfo {
            name,
            vendor: if has_extended {
                char8_array_to_string(&extended.vendor)
            } else {
                String::new()
            },
            version: 0,
            category,
            path: bundle.to_path_buf(),
            unique_id: tuid_to_hex(&info.cid),
            format: FORMAT,
            has_editor: false,
            accepts_midi: category == PluginCategory::Instrument,
        });
    }
    Ok(())
}

fn class_category(subcategories: &str) -> PluginCategory {
    if subcategories
        .split('|')
        .any(|category| category == "Instrument")
    {
        PluginCategory::Instrument
    } else {
        PluginCategory::Effect
    }
}

fn empty_pclass_info() -> PClassInfo {
    PClassInfo {
        cid: [0; 16],
        cardinality: 0,
        category: [0; PClassInfo_::kCategorySize as usize],
        name: [0; PClassInfo_::kNameSize as usize],
    }
}

#[cfg(test)]
mod tests;
