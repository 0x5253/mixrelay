//! Thin per-format adapters: the only module that names a plug-in format crate.
//!
//! Each function picks the format's `truce-rack` scanner and hands it to format-neutral host
//! code. A format compiled out of this target fails without loading anything.

use std::path::PathBuf;

use mixrelay_audio_graph::ProviderPorts;
use truce_rack_core::{Plugin, info::PluginInfo, scanner::PluginScanner};

use super::{
    identity::{RackFormat, RackLocator},
    probe::{self, RackProbeRecord},
    scan::{self, ScanPathFailure},
};

/// Formats available in the current target build.
#[must_use]
pub(crate) fn supported_formats() -> Vec<RackFormat> {
    #[cfg(target_vendor = "apple")]
    let formats = vec![
        RackFormat::Clap,
        RackFormat::Vst3,
        RackFormat::Au,
        RackFormat::Auv3,
    ];
    #[cfg(target_os = "linux")]
    let formats = vec![RackFormat::Clap, RackFormat::Vst3, RackFormat::Lv2];
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    let formats = vec![RackFormat::Clap, RackFormat::Vst3];
    formats
}

/// Discovers `format`'s plug-ins. Only filesystem-based formats (CLAP, VST3) scan custom paths.
pub(super) fn scan(
    format: RackFormat,
    custom_paths: &[PathBuf],
) -> Result<(Vec<PluginInfo>, Vec<ScanPathFailure>), String> {
    match format {
        RackFormat::Clap => {
            scan_with_custom_paths(format, &truce_rack_clap::ClapScanner::new(), custom_paths)
        }
        RackFormat::Vst3 => {
            scan_with_custom_paths(format, &truce_rack_vst3::Vst3Scanner::new(), custom_paths)
        }
        #[cfg(target_vendor = "apple")]
        RackFormat::Au => scan_defaults(&truce_rack_au::AuScanner::new()),
        #[cfg(target_vendor = "apple")]
        RackFormat::Auv3 => scan_defaults(&truce_rack_au3::Au3Scanner::new()),
        #[cfg(target_os = "linux")]
        RackFormat::Lv2 => scan_defaults(&truce_rack_lv2::Lv2Scanner::new()),
        #[allow(unreachable_patterns)]
        _ => Err(unavailable(format)),
    }
}

fn scan_with_custom_paths(
    format: RackFormat,
    scanner: &impl PluginScanner,
    custom_paths: &[PathBuf],
) -> Result<(Vec<PluginInfo>, Vec<ScanPathFailure>), String> {
    scan::scan_with_paths(
        format,
        custom_paths,
        || scanner.scan().map_err(|error| error.to_string()),
        |path| scanner.scan_path(path).map_err(|error| error.to_string()),
    )
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn scan_defaults(
    scanner: &impl PluginScanner,
) -> Result<(Vec<PluginInfo>, Vec<ScanPathFailure>), String> {
    let infos = scanner.scan().map_err(|error| error.to_string())?;
    Ok((infos, Vec::new()))
}

/// Loads `locator` with its format's scanner and describes its verified layouts.
pub(super) fn probe(locator: RackLocator) -> Result<RackProbeRecord, String> {
    match locator.format {
        RackFormat::Clap => probe_with(&truce_rack_clap::ClapScanner::new(), locator),
        RackFormat::Vst3 => probe_with(&truce_rack_vst3::Vst3Scanner::new(), locator),
        #[cfg(target_vendor = "apple")]
        RackFormat::Au => probe_with(&truce_rack_au::AuScanner::new(), locator),
        #[cfg(target_vendor = "apple")]
        RackFormat::Auv3 => probe_with(&truce_rack_au3::Au3Scanner::new(), locator),
        #[cfg(target_os = "linux")]
        RackFormat::Lv2 => probe_with(&truce_rack_lv2::Lv2Scanner::new(), locator),
        #[allow(unreachable_patterns)]
        _ => Err(unavailable(locator.format)),
    }
}

fn probe_with<S>(scanner: &S, locator: RackLocator) -> Result<RackProbeRecord, String>
where
    S: PluginScanner,
    S::Plugin: Plugin<f32> + 'static,
{
    let mut plugin = scanner
        .load(&locator.info())
        .map_err(|error| error.to_string())?;
    probe::describe_loaded_plugin(locator, &mut plugin)
}

/// Loads a worker instance. Audio Units take the selected bus layout at load time; other
/// formats receive it at activation.
pub(super) fn load_worker_plugin(
    locator: &RackLocator,
    selected: &ProviderPorts,
) -> Result<Box<dyn Plugin<f32>>, String> {
    #[cfg(not(target_vendor = "apple"))]
    let _ = selected;
    let info = locator.info();
    match locator.format {
        RackFormat::Vst3 => boxed(truce_rack_vst3::Vst3Scanner::new().load(&info)),
        RackFormat::Clap => boxed(truce_rack_clap::ClapScanner::new().load(&info)),
        #[cfg(target_vendor = "apple")]
        RackFormat::Au | RackFormat::Auv3 => boxed(
            truce_rack_au::AuScanner::new()
                .load_with_layout(&info, &super::layouts::selected_au_layout(selected)?),
        ),
        #[cfg(target_os = "linux")]
        RackFormat::Lv2 => boxed(truce_rack_lv2::Lv2Scanner::new().load(&info)),
        #[allow(unreachable_patterns)]
        _ => Err(unavailable(locator.format)),
    }
}

fn boxed<P: Plugin<f32> + 'static>(
    loaded: truce_rack_core::Result<P>,
) -> Result<Box<dyn Plugin<f32>>, String> {
    loaded
        .map(|plugin| Box::new(plugin) as Box<dyn Plugin<f32>>)
        .map_err(|error| error.to_string())
}

fn unavailable(format: RackFormat) -> String {
    format!("{} is unavailable on this platform", format.argument())
}
