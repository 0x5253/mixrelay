//! The disposable scanner child: per-format discovery and its custom-path policy.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use truce_rack_core::info::PluginInfo;

use super::{
    adapters,
    identity::{RackFormat, RackLocator},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RackScanReport {
    pub locators: Vec<RackLocator>,
    pub path_failures: Vec<ScanPathFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ScanPathFailure {
    pub path: PathBuf,
    pub message: String,
}

/// Runs one format scanner in a child process and prints serializable plugin identities.
///
/// # Errors
///
/// Returns an error when discovery fails or its result cannot be serialized.
pub fn run_scan(format: RackFormat, custom_paths: &[PathBuf]) -> Result<(), String> {
    let _parent = super::enter_child_process()?;
    let (infos, path_failures) = adapters::scan(format, custom_paths)?;
    let locators = infos
        .into_iter()
        .map(|info| RackLocator::from_info(format, info))
        .filter(RackLocator::is_audio_processor)
        .collect();
    super::print_json(&RackScanReport {
        locators,
        path_failures,
    })
}

/// Combines the default scan with each custom path. A failed default scan fails the whole
/// format; a failed custom path is reported without discarding other results.
pub(super) fn scan_with_paths(
    format: RackFormat,
    custom_paths: &[PathBuf],
    default_scan: impl FnOnce() -> Result<Vec<PluginInfo>, String>,
    mut scan_path: impl FnMut(&Path) -> Result<Vec<PluginInfo>, String>,
) -> Result<(Vec<PluginInfo>, Vec<ScanPathFailure>), String> {
    let mut infos = default_scan()?;
    let mut path_failures = Vec::new();
    for path in custom_paths {
        if is_other_format_bundle(path, format) {
            continue;
        }
        match scan_path(path) {
            Ok(found) => infos.extend(found),
            Err(error) => path_failures.push(ScanPathFailure {
                path: path.clone(),
                message: error,
            }),
        }
    }
    Ok((infos, path_failures))
}

fn is_other_format_bundle(path: &Path, format: RackFormat) -> bool {
    let extension = path.extension().and_then(std::ffi::OsStr::to_str);
    matches!(extension, Some("clap" | "vst3" | "lv2")) && extension != Some(format.argument())
}

/// `LV2_PATH` for an LV2 scanner child: custom paths first, then the inherited or default
/// search path. `None` on targets without LV2.
#[must_use]
pub(crate) fn lv2_search_path(custom_paths: &[PathBuf]) -> Option<std::ffi::OsString> {
    #[cfg(target_os = "linux")]
    {
        let mut paths = custom_paths.to_vec();
        if let Some(existing) = std::env::var_os("LV2_PATH") {
            paths.extend(std::env::split_paths(&existing));
        } else if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join(".lv2"));
            paths.extend([
                PathBuf::from("/usr/local/lib/lv2"),
                PathBuf::from("/usr/lib/lv2"),
            ]);
        }
        std::env::join_paths(paths).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = custom_paths;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use truce_rack_core::info::PluginCategory;

    fn info(id: &str) -> PluginInfo {
        PluginInfo {
            name: id.to_owned(),
            vendor: "fixture".to_owned(),
            version: 1,
            category: PluginCategory::Effect,
            path: PathBuf::from(format!("/fixture/{id}")),
            unique_id: id.to_owned(),
            format: "clap",
            has_editor: false,
            accepts_midi: false,
        }
    }

    fn ids(infos: &[PluginInfo]) -> Vec<&str> {
        infos.iter().map(|info| info.unique_id.as_str()).collect()
    }

    #[test]
    fn custom_path_failure_keeps_default_and_other_successful_scan_results() {
        let good = PathBuf::from("/good");
        let bad = PathBuf::from("/bad");

        let (infos, failures) = scan_with_paths(
            RackFormat::Clap,
            std::slice::from_ref(&bad),
            || Ok(vec![info("default")]),
            |_| Err("permission denied".into()),
        )
        .unwrap();
        assert_eq!(ids(&infos), ["default"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].path, bad);
        assert_eq!(failures[0].message, "permission denied");

        let (infos, failures) = scan_with_paths(
            RackFormat::Clap,
            &[good.clone(), bad.clone()],
            || Ok(vec![info("default")]),
            |path| {
                if path == good.as_path() {
                    Ok(vec![info("custom")])
                } else {
                    Err("path unavailable".into())
                }
            },
        )
        .unwrap();
        assert_eq!(ids(&infos), ["default", "custom"]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].path, bad);
        assert_eq!(failures[0].message, "path unavailable");
    }

    #[test]
    fn default_scan_failure_fails_the_format_before_custom_paths_run() {
        let error = scan_with_paths(
            RackFormat::Clap,
            &[PathBuf::from("/good"), PathBuf::from("/bad")],
            || Err("default scan failed".into()),
            |_| panic!("custom paths must not run after whole-format failure"),
        )
        .unwrap_err();
        assert_eq!(error, "default scan failed");
    }

    #[test]
    fn custom_bundles_reach_only_their_own_format_while_directories_reach_every_format() {
        let paths = [
            PathBuf::from("/fixture/Other.vst3"),
            PathBuf::from("/fixture/Own.clap"),
            PathBuf::from("/fixture/Plug-Ins"),
        ];
        let mut scanned = Vec::new();
        let (infos, failures) = scan_with_paths(
            RackFormat::Clap,
            &paths,
            || Ok(vec![info("default")]),
            |path| {
                scanned.push(path.to_path_buf());
                Ok(Vec::new())
            },
        )
        .unwrap();
        assert_eq!(ids(&infos), ["default"]);
        assert!(failures.is_empty());
        assert_eq!(scanned, paths[1..]);
    }
}
