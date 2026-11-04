//! The disposable probe child: layout negotiation and the catalog descriptor of one candidate.

use mixrelay_audio_graph::ProviderPorts;
use mixrelay_control_protocol::{ExternalProcessorDescriptor, ProcessorParameterDescriptor};
use serde::{Deserialize, Serialize};
use truce_rack_core::{Plugin, info::ParameterFlags};

use super::{adapters, effect_layout, identity::RackLocator, layouts};

/// Probe activation settings. Worker activation uses the engine's real rate and block size.
const PROBE_SAMPLE_RATE: f64 = 48_000.0;
const PROBE_BLOCK_FRAMES: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RackProbeRecord {
    pub descriptor: ExternalProcessorDescriptor,
    pub locator: RackLocator,
}

/// Loads a single candidate in an isolated process and emits its usable effect layout.
///
/// # Errors
///
/// Returns an error when the processor cannot load, activate, or expose a supported layout.
pub fn run_probe(locator: RackLocator) -> Result<(), String> {
    let _parent = super::enter_child_process()?;
    super::print_json(&adapters::probe(locator)?)
}

/// Verifies every layout of a loaded candidate, then activates its default layout once more
/// and describes it. Nothing is published unless the instance deactivates afterwards.
pub(super) fn describe_loaded_plugin<P: Plugin<f32>>(
    mut locator: RackLocator,
    plugin: &mut P,
) -> Result<RackProbeRecord, String> {
    let verified = layouts::verified_layouts(plugin)?;
    let default_layout =
        effect_layout::default_verified_layout(plugin.supported_layouts(), &verified)?;
    let default_ports = layouts::describe(&default_layout)?;
    // A successful scan only proves that the binary can be opened. Activate the selected layout
    // in the disposable probe process so an incompatible plugin never reaches the live catalog.
    plugin
        .activate(default_layout, PROBE_SAMPLE_RATE, PROBE_BLOCK_FRAMES)
        .map_err(|error| error.to_string())?;
    // Some formats (notably Audio Unit and CLAP) can only report native-editor support after
    // loading an instance. The scan locator is therefore a hint; the disposable probe is the
    // authoritative capability source exposed to both the UI catalog and the live worker.
    locator.has_editor = plugin.info().has_editor;
    let record = layouts::verify_active(plugin, &default_ports).map(|()| RackProbeRecord {
        descriptor: descriptor(
            &locator,
            verified,
            default_ports,
            visible_parameters(plugin),
        ),
        locator,
    });
    plugin.deactivate();
    if plugin.is_active() {
        return Err("processor default layout did not deactivate".into());
    }
    record
}

fn descriptor(
    locator: &RackLocator,
    layouts: Vec<ProviderPorts>,
    default_ports: ProviderPorts,
    parameters: Vec<ProcessorParameterDescriptor>,
) -> ExternalProcessorDescriptor {
    ExternalProcessorDescriptor {
        type_id: format!("{}:{}", locator.format.argument(), locator.unique_id),
        display_name: locator.name.clone(),
        vendor: locator.vendor.clone(),
        version: locator.version.to_string(),
        format: locator.format.protocol(),
        layouts,
        default_layout_id: default_ports.layout_id,
        has_editor: locator.has_editor,
        parameters,
    }
}

/// Readable, non-hidden parameters; a parameter whose metadata fails to load is omitted.
fn visible_parameters<P: Plugin<f32>>(plugin: &P) -> Vec<ProcessorParameterDescriptor> {
    (0..plugin.parameter_count())
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
        .collect()
}

fn normalize_parameter(value: f64, minimum: f64, maximum: f64) -> f64 {
    if !value.is_finite() || !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
        return 0.0;
    }
    ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests;
