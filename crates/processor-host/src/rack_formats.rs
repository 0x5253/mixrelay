//! Format adapters built on the shared `truce-rack` host contract.
//!
//! Discovery and probing run only in disposable child processes. A third-party binary can
//! therefore crash or hang without taking down the graph/control process. DSP instances run in
//! the same bounded shared-memory worker model for every supported format.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mixrelay_audio_graph::ProviderPorts;
use mixrelay_control_protocol::{
    ExternalProcessorDescriptor, ExternalProcessorFormat, ProcessorParameterDescriptor,
};
use serde::{Deserialize, Serialize};
use truce_rack_core::{
    Plugin,
    bus::BusLayout,
    info::{ParameterFlags, PluginCategory, PluginInfo},
    scanner::PluginScanner,
};

mod worker;
pub(crate) use worker::RackWorkerPlugin;
pub(crate) mod layouts;

pub const SCAN_ARGUMENT: &str = "--mixrelay-plugin-scan";
pub const PROBE_ARGUMENT: &str = "--mixrelay-plugin-probe";
pub const WORKER_ARGUMENT: &str = "--mixrelay-plugin-worker";

const MAX_CHANNELS: u16 = 64;
const CHANNEL_SLOTS: usize = MAX_CHANNELS as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RackFormat {
    Au,
    Auv3,
    Clap,
    Lv2,
    Vst3,
}

impl RackFormat {
    #[must_use]
    pub const fn argument(self) -> &'static str {
        match self {
            Self::Au => "au",
            Self::Auv3 => "auv3",
            Self::Clap => "clap",
            Self::Lv2 => "lv2",
            Self::Vst3 => "vst3",
        }
    }

    /// Parses the stable command-line representation used by scanner children.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested format is unknown to this host build.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "au" => Ok(Self::Au),
            "auv3" => Ok(Self::Auv3),
            "clap" => Ok(Self::Clap),
            "lv2" => Ok(Self::Lv2),
            "vst3" => Ok(Self::Vst3),
            _ => Err(format!("unsupported processor format: {value}")),
        }
    }

    pub(crate) const fn protocol(self) -> ExternalProcessorFormat {
        match self {
            Self::Au => ExternalProcessorFormat::Au,
            Self::Auv3 => ExternalProcessorFormat::Auv3,
            Self::Clap => ExternalProcessorFormat::Clap,
            Self::Lv2 => ExternalProcessorFormat::Lv2,
            Self::Vst3 => ExternalProcessorFormat::Vst3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RackCategory {
    Effect,
    Instrument,
    NoteEffect,
    Analyzer,
    Tool,
}

impl From<PluginCategory> for RackCategory {
    fn from(value: PluginCategory) -> Self {
        match value {
            PluginCategory::Effect => Self::Effect,
            PluginCategory::Instrument => Self::Instrument,
            PluginCategory::NoteEffect => Self::NoteEffect,
            PluginCategory::Analyzer => Self::Analyzer,
            PluginCategory::Tool => Self::Tool,
        }
    }
}

impl From<RackCategory> for PluginCategory {
    fn from(value: RackCategory) -> Self {
        match value {
            RackCategory::Effect => Self::Effect,
            RackCategory::Instrument => Self::Instrument,
            RackCategory::NoteEffect => Self::NoteEffect,
            RackCategory::Analyzer => Self::Analyzer,
            RackCategory::Tool => Self::Tool,
        }
    }
}

/// Serializable identity passed between the scanner, probe, and DSP worker processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RackLocator {
    pub format: RackFormat,
    pub unique_id: String,
    pub path: PathBuf,
    name: String,
    vendor: String,
    version: u32,
    category: RackCategory,
    has_editor: bool,
    accepts_midi: bool,
}

impl RackLocator {
    fn from_info(format: RackFormat, info: PluginInfo) -> Self {
        Self {
            format,
            unique_id: info.unique_id,
            path: info.path,
            name: info.name,
            vendor: info.vendor,
            version: info.version,
            category: info.category.into(),
            has_editor: info.has_editor,
            accepts_midi: info.accepts_midi,
        }
    }

    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.name.clone(),
            vendor: self.vendor.clone(),
            version: self.version,
            category: self.category.into(),
            path: self.path.clone(),
            unique_id: self.unique_id.clone(),
            format: self.format.argument(),
            has_editor: self.has_editor,
            accepts_midi: self.accepts_midi,
        }
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.name
    }

    pub(super) fn is_audio_processor(&self) -> bool {
        self.category == RackCategory::Effect
            && match self.format {
                // Audio Unit generators and instruments can expose nominal stereo buses even
                // though they are not insert effects. Restrict the processor menu to the native
                // effect and music-effect component families.
                RackFormat::Au | RackFormat::Auv3 => {
                    self.unique_id.starts_with("aufx:") || self.unique_id.starts_with("aumf:")
                }
                RackFormat::Clap | RackFormat::Lv2 | RackFormat::Vst3 => true,
            }
    }

    /// Serializes this identity for a worker command-line argument.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity cannot be serialized.
    pub fn encode(&self) -> Result<String, String> {
        serde_json::to_vec(self)
            .map(|bytes| BASE64.encode(bytes))
            .map_err(|error| error.to_string())
    }

    /// Restores a worker identity from its command-line representation.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed Base64 or invalid serialized identity data.
    pub fn decode(encoded: &str) -> Result<Self, String> {
        let bytes = BASE64.decode(encoded).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RackProbeRecord {
    pub descriptor: ExternalProcessorDescriptor,
    pub locator: RackLocator,
}

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

/// Runs one format scanner in a child process and prints serializable plugin identities.
///
/// # Errors
///
/// Returns an error when discovery fails or its result cannot be serialized.
pub fn run_scan(format: RackFormat, custom_paths: &[PathBuf]) -> Result<(), String> {
    let _parent = crate::dsp_lifecycle::watch_parent(std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    ))
    .map_err(|error| error.to_string())?;
    crate::worker::prepare_plugin_process()?;
    let (infos, path_failures) = match format {
        RackFormat::Clap => {
            let scanner = truce_rack_clap::ClapScanner::new();
            scan_with_paths(
                format,
                custom_paths,
                || scanner.scan().map_err(|error| error.to_string()),
                |path| scanner.scan_path(path).map_err(|error| error.to_string()),
            )?
        }
        RackFormat::Vst3 => {
            let scanner = truce_rack_vst3::Vst3Scanner::new();
            scan_with_paths(
                format,
                custom_paths,
                || scanner.scan().map_err(|error| error.to_string()),
                |path| scanner.scan_path(path).map_err(|error| error.to_string()),
            )?
        }
        #[cfg(target_vendor = "apple")]
        RackFormat::Au => (
            truce_rack_au::AuScanner::new()
                .scan()
                .map_err(|error| error.to_string())?,
            Vec::new(),
        ),
        #[cfg(target_vendor = "apple")]
        RackFormat::Auv3 => (
            truce_rack_au3::Au3Scanner::new()
                .scan()
                .map_err(|error| error.to_string())?,
            Vec::new(),
        ),
        #[cfg(target_os = "linux")]
        RackFormat::Lv2 => (
            truce_rack_lv2::Lv2Scanner::new()
                .scan()
                .map_err(|error| error.to_string())?,
            Vec::new(),
        ),
        #[allow(unreachable_patterns)]
        _ => {
            return Err(format!(
                "{} is unavailable on this platform",
                format.argument()
            ));
        }
    };
    let locators = infos
        .into_iter()
        .map(|info| RackLocator::from_info(format, info))
        .filter(RackLocator::is_audio_processor)
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string(&RackScanReport {
            locators,
            path_failures,
        })
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn scan_with_paths(
    format: RackFormat,
    custom_paths: &[PathBuf],
    default_scan: impl FnOnce() -> Result<Vec<PluginInfo>, String>,
    mut scan_path: impl FnMut(&Path) -> Result<Vec<PluginInfo>, String>,
) -> Result<(Vec<PluginInfo>, Vec<ScanPathFailure>), String> {
    let mut infos = default_scan()?;
    let mut path_failures = Vec::new();
    for path in custom_paths {
        let extension = path.extension().and_then(std::ffi::OsStr::to_str);
        if matches!(extension, Some("clap" | "vst3" | "lv2"))
            && extension != Some(format.argument())
        {
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

/// Loads a single candidate in an isolated process and emits its usable effect layout.
///
/// # Errors
///
/// Returns an error when the processor cannot load, activate, or expose a supported layout.
pub fn run_probe(locator: RackLocator) -> Result<(), String> {
    let _parent = crate::dsp_lifecycle::watch_parent(std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    ))
    .map_err(|error| error.to_string())?;
    crate::worker::prepare_plugin_process()?;
    let record = probe(locator)?;
    println!(
        "{}",
        serde_json::to_string(&record).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn probe(locator: RackLocator) -> Result<RackProbeRecord, String> {
    match locator.format {
        RackFormat::Clap => probe_with_scanner(&truce_rack_clap::ClapScanner::new(), locator),
        RackFormat::Vst3 => probe_with_scanner(&truce_rack_vst3::Vst3Scanner::new(), locator),
        #[cfg(target_vendor = "apple")]
        RackFormat::Au => probe_with_scanner(&truce_rack_au::AuScanner::new(), locator),
        #[cfg(target_vendor = "apple")]
        RackFormat::Auv3 => probe_with_scanner(&truce_rack_au3::Au3Scanner::new(), locator),
        #[cfg(target_os = "linux")]
        RackFormat::Lv2 => probe_with_scanner(&truce_rack_lv2::Lv2Scanner::new(), locator),
        #[allow(unreachable_patterns)]
        _ => Err(format!(
            "{} is unavailable on this platform",
            locator.format.argument()
        )),
    }
}

fn probe_with_scanner<S>(scanner: &S, locator: RackLocator) -> Result<RackProbeRecord, String>
where
    S: PluginScanner,
    S::Plugin: Plugin<f32> + 'static,
{
    let mut plugin = scanner
        .load(&locator.info())
        .map_err(|error| error.to_string())?;
    describe_loaded_plugin(locator, &mut plugin)
}

fn describe_loaded_plugin<P: Plugin<f32>>(
    mut locator: RackLocator,
    plugin: &mut P,
) -> Result<RackProbeRecord, String> {
    let verified = layouts::verified_layouts(plugin)?;
    let usable_ids: HashSet<&str> = verified
        .iter()
        .map(|ports| ports.layout_id.as_str())
        .collect();
    let usable: Vec<_> = plugin
        .supported_layouts()
        .iter()
        .filter(|layout| {
            layouts::describe(layout)
                .is_ok_and(|ports| usable_ids.contains(ports.layout_id.as_str()))
        })
        .cloned()
        .collect();
    let (_, layout) = select_effect_layout(&usable)
        .ok_or_else(|| "plugin exposes no supported audio effect layout".to_owned())?;
    let layout = layout.clone();
    let default_ports = layouts::describe(&layout)?;
    let default_layout_id = default_ports.layout_id.clone();
    // A successful scan only proves that the binary can be opened. Activate the selected layout
    // in the disposable probe process so an incompatible plugin never reaches the live catalog.
    plugin
        .activate(layout.clone(), 48_000.0, 128)
        .map_err(|error| error.to_string())?;
    // Some formats (notably Audio Unit and CLAP) can only report native-editor support after
    // loading an instance. The scan locator is therefore a hint; the disposable probe is the
    // authoritative capability source exposed to both the UI catalog and the live worker.
    locator.has_editor = plugin.info().has_editor;
    let result = (|| {
        layouts::verify_active(plugin, &default_ports)?;
        let parameters = (0..plugin.parameter_count())
            .filter_map(|index| plugin.parameter_info(index).ok())
            .filter(|parameter| !parameter.flags.contains(ParameterFlags::HIDDEN))
            .map(|parameter| ProcessorParameterDescriptor {
                id: parameter.id,
                name: parameter.name,
                default_value: normalize_parameter(parameter.default, parameter.min, parameter.max),
                step_count: i32::try_from(parameter.step_count).unwrap_or(i32::MAX),
                can_automate: parameter.flags.contains(ParameterFlags::AUTOMATABLE),
                is_read_only: parameter.flags.contains(ParameterFlags::READ_ONLY),
            })
            .collect();
        Ok(RackProbeRecord {
            descriptor: ExternalProcessorDescriptor {
                type_id: format!("{}:{}", locator.format.argument(), locator.unique_id),
                display_name: locator.name.clone(),
                vendor: locator.vendor.clone(),
                version: locator.version.to_string(),
                format: locator.format.protocol(),
                layouts: verified,
                default_layout_id,
                has_editor: locator.has_editor,
                parameters,
            },
            locator,
        })
    })();
    plugin.deactivate();
    if plugin.is_active() {
        return Err("processor default layout did not deactivate".into());
    }
    result
}

fn checked_channels(channels: u32) -> Result<u16, String> {
    u16::try_from(channels)
        .ok()
        .filter(|channels| (1..=MAX_CHANNELS).contains(channels))
        .ok_or_else(|| format!("plugin effect layout requires 1..={MAX_CHANNELS} channels"))
}

fn layout_channels(layout: &BusLayout) -> Result<(u16, u16), String> {
    fn total(buses: &[truce_rack_core::bus::Bus]) -> Result<u16, String> {
        let channels = buses
            .iter()
            .try_fold(0_u32, |total, bus| total.checked_add(bus.channels.count()));
        checked_channels(channels.unwrap_or(u32::MAX))
    }
    Ok((total(&layout.inputs)?, total(&layout.outputs)?))
}

fn select_effect_layout(layouts: &[BusLayout]) -> Option<(usize, &BusLayout)> {
    layouts
        .iter()
        .enumerate()
        .filter(|(_, layout)| layout_channels(layout).is_ok())
        .min_by_key(|(_, layout)| {
            let inputs = layout.total_input_channels();
            let outputs = layout.total_output_channels();
            let stereo_penalty = u32::from(inputs != 2 || outputs != 2);
            let asymmetric_penalty = u32::from(inputs != outputs);
            (stereo_penalty, asymmetric_penalty, inputs + outputs)
        })
}

fn normalize_parameter(value: f64, minimum: f64, maximum: f64) -> f64 {
    if !value.is_finite() || !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
        return 0.0;
    }
    ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
}

/// Format-erased plugin instance owned exclusively by one DSP worker process.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EditorSnapshot {
    pub open: bool,
    pub size: Option<(u32, u32)>,
    pub resizable: bool,
}

fn load_plugin(
    locator: &RackLocator,
    selected: &ProviderPorts,
) -> Result<Box<dyn Plugin<f32>>, String> {
    #[cfg(not(target_vendor = "apple"))]
    let _ = selected;
    let info = locator.info();
    match locator.format {
        RackFormat::Vst3 => truce_rack_vst3::Vst3Scanner::new()
            .load(&info)
            .map(|plugin| Box::new(plugin) as Box<dyn Plugin<f32>>)
            .map_err(|error| error.to_string()),
        RackFormat::Clap => truce_rack_clap::ClapScanner::new()
            .load(&info)
            .map(|plugin| Box::new(plugin) as Box<dyn Plugin<f32>>)
            .map_err(|error| error.to_string()),
        #[cfg(target_vendor = "apple")]
        RackFormat::Au | RackFormat::Auv3 => truce_rack_au::AuScanner::new()
            .load_with_layout(&info, &layouts::selected_au_layout(selected)?)
            .map(|plugin| Box::new(plugin) as Box<dyn Plugin<f32>>)
            .map_err(|error| error.to_string()),
        #[cfg(target_os = "linux")]
        RackFormat::Lv2 => truce_rack_lv2::Lv2Scanner::new()
            .load(&info)
            .map(|plugin| Box::new(plugin) as Box<dyn Plugin<f32>>)
            .map_err(|error| error.to_string()),
        #[allow(unreachable_patterns)]
        _ => Err(format!(
            "{} is unavailable on this platform",
            locator.format.argument()
        )),
    }
}

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

    fn scan_fixture_info(id: &str) -> PluginInfo {
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

    #[test]
    fn custom_path_failure_keeps_default_and_other_successful_scan_results() {
        let good = PathBuf::from("/good");
        let bad = PathBuf::from("/bad");
        let default = scan_fixture_info("default");
        let custom = scan_fixture_info("custom");
        let (infos, failures) = scan_with_paths(
            RackFormat::Clap,
            std::slice::from_ref(&bad),
            || Ok(vec![default.clone()]),
            |_| Err("permission denied".into()),
        )
        .unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].unique_id, "default");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].path, bad);
        assert_eq!(failures[0].message, "permission denied");

        let (infos, failures) = scan_with_paths(
            RackFormat::Clap,
            &[good.clone(), bad.clone()],
            || Ok(vec![default]),
            |path| {
                if path == good.as_path() {
                    Ok(vec![custom.clone()])
                } else {
                    Err("path unavailable".into())
                }
            },
        )
        .unwrap();
        assert_eq!(
            infos
                .iter()
                .map(|info| info.unique_id.as_str())
                .collect::<Vec<_>>(),
            ["default", "custom"]
        );
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].path, bad);
        assert_eq!(failures[0].message, "path unavailable");

        let error = scan_with_paths(
            RackFormat::Clap,
            &[good, bad],
            || Err("default scan failed".into()),
            |_| panic!("custom paths must not run after whole-format failure"),
        )
        .unwrap_err();
        assert_eq!(error, "default scan failed");

        let foreign_bundle = PathBuf::from("/fixture/Other.vst3");
        let (infos, failures) = scan_with_paths(
            RackFormat::Clap,
            &[foreign_bundle],
            || Ok(vec![scan_fixture_info("default")]),
            |_| panic!("another format's exact bundle is not a CLAP scan path"),
        )
        .unwrap();
        assert_eq!(infos.len(), 1);
        assert!(failures.is_empty());
    }

    #[cfg(target_os = "macos")]
    fn apple_limiter_locator() -> RackLocator {
        let info = truce_rack_au::AuScanner::new()
            .scan()
            .unwrap()
            .into_iter()
            .find(|info| info.name.contains("AUPeakLimiter"))
            .expect("macOS ships the AUPeakLimiter fixture");
        RackLocator::from_info(RackFormat::Au, info)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_limiter_native_state_restores_a_changed_parameter() {
        // The built-in AU fixture requires neither hardware nor a vendor authorization. This
        // checks native ClassInfo bytes, not a mock JSON parameter round trip.
        let locator = apple_limiter_locator();
        let record = probe(locator.clone()).unwrap();
        let parameter = record
            .descriptor
            .parameters
            .iter()
            .find(|parameter| !parameter.is_read_only)
            .expect("limiter exposes a writable parameter");
        let layout = record
            .descriptor
            .layouts
            .iter()
            .find(|layout| layout.layout_id == record.descriptor.default_layout_id)
            .unwrap();
        let mut first = RackWorkerPlugin::load(
            &locator,
            layout,
            48_000.0,
            128,
            &crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap(),
        )
        .unwrap();
        first.set_parameter(parameter.id, 0.37).unwrap();
        let range = first
            .parameter_ranges
            .iter()
            .find(|range| range.id == parameter.id)
            .copied()
            .unwrap();
        let expected = first.plugin.parameter_value(range.index).unwrap();
        let first = std::sync::Mutex::new(first);
        let state = snapshot_native_fixture_state(&first);
        assert!(!state.is_empty());
        first
            .lock()
            .unwrap()
            .set_parameter(parameter.id, 0.91)
            .unwrap();
        let restored = RackWorkerPlugin::load(
            &locator,
            layout,
            48_000.0,
            128,
            &crate::state::InitialState {
                parameters: serde_json::json!({}),
                native: Some(state),
            },
        )
        .unwrap();
        let actual = restored.plugin.parameter_value(range.index).unwrap();
        assert!(
            (actual - expected).abs() < 0.001,
            "native state failed to restore {expected}: {actual}"
        );
    }

    #[cfg(target_os = "macos")]
    fn snapshot_native_fixture_state(plugin: &std::sync::Mutex<RackWorkerPlugin>) -> Vec<u8> {
        let paths = crate::worker_paths::WorkerPaths::new_in(&std::env::temp_dir())
            .expect("private worker paths");
        let shared_path = paths.shared().to_path_buf();
        let config_path = paths.config().to_path_buf();
        let (audio, control) = crate::shared_transport::create_owned(
            &paths,
            crate::AudioLayout {
                inputs: 2,
                outputs: 2,
                block_frames: 128,
            },
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .unwrap();
        drop(audio);
        let control = crate::HostedProcessorControl::native(
            control,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        );
        let (_, shared) = crate::shared_transport::worker(&shared_path).unwrap();
        shared
            .header()
            .worker_status
            .store(crate::WORKER_READY, std::sync::atomic::Ordering::Release);
        let client =
            std::thread::spawn(move || control.snapshot_state(std::time::Duration::from_secs(2)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !client.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            crate::worker::broker::apply_pending_state_request(&shared, plugin);
            std::thread::yield_now();
        }
        let state = client.join().unwrap().unwrap();
        drop(shared);
        crate::worker_process::cleanup_worker_paths(&shared_path, &config_path);
        assert!(!shared_path.exists());
        state
    }

    #[test]
    fn effect_layout_prefers_plain_stereo() {
        let layouts = [
            BusLayout::stereo_with_sidechain("Sidechain"),
            BusLayout::mono(),
            BusLayout::stereo(),
        ];

        let (index, selected) = select_effect_layout(&layouts).expect("one layout is usable");

        assert_eq!(index, 2);
        assert_eq!(selected.total_input_channels(), 2);
        assert_eq!(selected.total_output_channels(), 2);
    }

    #[test]
    fn effect_layout_rejects_empty_and_over_limit_layouts() {
        use truce_rack_core::bus::{Bus, ChannelConfig};

        let mut over_limit = BusLayout::new();
        over_limit
            .inputs
            .push(Bus::main("Input", ChannelConfig::Discrete(65)));
        over_limit
            .outputs
            .push(Bus::main("Output", ChannelConfig::Discrete(65)));

        assert!(select_effect_layout(&[BusLayout::new(), over_limit]).is_none());
    }

    #[test]
    fn parameter_normalization_is_bounded_and_handles_invalid_ranges() {
        assert!((normalize_parameter(0.0, -1.0, 1.0) - 0.5).abs() < f64::EPSILON);
        assert!(normalize_parameter(-4.0, -1.0, 1.0).abs() < f64::EPSILON);
        assert!((normalize_parameter(4.0, -1.0, 1.0) - 1.0).abs() < f64::EPSILON);
        assert!(normalize_parameter(1.0, 1.0, 1.0).abs() < f64::EPSILON);
        assert!(normalize_parameter(f64::NAN, 0.0, 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn locator_round_trips_through_worker_argument() {
        let directory = mixrelay_test_support::FixtureDir::new("rack-locator").unwrap();
        let locator = RackLocator {
            format: RackFormat::Clap,
            unique_id: "io.mixrelay.fixture".to_owned(),
            path: directory.join("MixRelay Fixture.clap"),
            name: "Fixture".to_owned(),
            vendor: "MixRelay".to_owned(),
            version: 3,
            category: RackCategory::Effect,
            has_editor: true,
            accepts_midi: false,
        };

        let decoded = RackLocator::decode(&locator.encode().expect("locator should encode"))
            .expect("locator should decode");

        assert_eq!(decoded.format, locator.format);
        assert_eq!(decoded.unique_id, locator.unique_id);
        assert_eq!(decoded.path, locator.path);
        assert_eq!(decoded.name, locator.name);
        assert_eq!(decoded.vendor, locator.vendor);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_bandpass_editor_capability_reaches_catalog_and_worker() {
        let info = truce_rack_au::AuScanner::new()
            .scan()
            .unwrap()
            .into_iter()
            .find(|info| info.name.contains("AUBandpass"))
            .expect("macOS ships the AUBandpass fixture");
        let record = probe(RackLocator::from_info(RackFormat::Au, info)).unwrap();
        assert!(record.descriptor.has_editor);
        assert!(record.locator.has_editor);
        let worker = RackWorkerPlugin::load(
            &record.locator,
            record
                .descriptor
                .layouts
                .iter()
                .find(|layout| layout.layout_id == record.descriptor.default_layout_id)
                .unwrap(),
            48_000.0,
            128,
            &crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap(),
        )
        .unwrap();
        assert!(worker.has_editor());
    }
}
