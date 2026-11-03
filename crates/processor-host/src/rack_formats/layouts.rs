//! Validated, native-configuration-aware topology identities shared by probe and worker.

use mixrelay_audio_graph::{
    PortId, ProviderBus, ProviderBusKind, ProviderChannelFormat, ProviderPorts,
};
use sha2::{Digest, Sha256};
use truce_rack_core::bus::{Bus, BusKind, BusLayout, ChannelConfig};

const MAX_NATIVE_ID_BYTES: usize = 1024;

pub(super) fn describe(layout: &BusLayout) -> Result<ProviderPorts, String> {
    if layout.id.as_ref().is_some_and(|id| {
        id.is_empty() || id.len() > MAX_NATIVE_ID_BYTES || id.chars().any(char::is_control)
    }) {
        return Err("processor native layout identity is invalid".into());
    }
    let inputs = describe_buses(&layout.inputs)?;
    let outputs = describe_buses(&layout.outputs)?;
    let signature = serde_json::to_vec(&(&layout.id, geometry(&inputs), geometry(&outputs)))
        .map_err(|error| error.to_string())?;
    let ports = ProviderPorts {
        layout_id: format!("layout-v1:{:x}", Sha256::digest(signature)),
        inputs,
        outputs,
    };
    ports.validate().map_err(|error| error.to_string())?;
    Ok(ports)
}

fn describe_buses(buses: &[Bus]) -> Result<Vec<ProviderBus>, String> {
    if buses.len() > 64 {
        return Err("processor bus count exceeds the 64-channel boundary".into());
    }
    buses
        .iter()
        .enumerate()
        .map(|(index, bus)| {
            Ok(ProviderBus {
                id: PortId::from(
                    if index == 0 {
                        "audio".to_owned()
                    } else {
                        format!("bus:{index}")
                    }
                    .as_str(),
                ),
                name: bus.name.clone(),
                kind: match bus.kind {
                    BusKind::Main => ProviderBusKind::Main,
                    BusKind::Sidechain => ProviderBusKind::Sidechain,
                    BusKind::Auxiliary => ProviderBusKind::Auxiliary,
                },
                channels: u16::try_from(bus.channels.count())
                    .map_err(|_| "processor bus width exceeds the channel boundary")?,
                channel_format: match bus.channels {
                    ChannelConfig::Mono => ProviderChannelFormat::Mono,
                    ChannelConfig::Stereo => ProviderChannelFormat::Stereo,
                    ChannelConfig::Surround5_1 => ProviderChannelFormat::Surround5_1,
                    ChannelConfig::Surround7_1 => ProviderChannelFormat::Surround7_1,
                    ChannelConfig::Discrete(_) => ProviderChannelFormat::Discrete,
                },
            })
        })
        .collect()
}

fn geometry(buses: &[ProviderBus]) -> Vec<(&ProviderBusKind, &ProviderChannelFormat, u16)> {
    buses
        .iter()
        .map(|bus| (&bus.kind, &bus.channel_format, bus.channels))
        .collect()
}

pub(crate) fn same_topology(left: &ProviderPorts, right: &ProviderPorts) -> bool {
    left.layout_id == right.layout_id
        && geometry(&left.inputs) == geometry(&right.inputs)
        && geometry(&left.outputs) == geometry(&right.outputs)
        && left
            .inputs
            .iter()
            .map(|bus| &bus.id)
            .eq(right.inputs.iter().map(|bus| &bus.id))
        && left
            .outputs
            .iter()
            .map(|bus| &bus.id)
            .eq(right.outputs.iter().map(|bus| &bus.id))
}

pub(super) fn native_layout(
    layouts: &[BusLayout],
    selected: &ProviderPorts,
) -> Result<BusLayout, String> {
    selected.validate().map_err(|error| error.to_string())?;
    for layout in layouts {
        if let Ok(actual) = describe(layout)
            && same_topology(&actual, selected)
        {
            return Ok(layout.clone());
        }
    }
    Err("processor selected bus configuration is no longer available".into())
}

/// Reconstruct an AU selection without inventing a native ID or changing ordered bus geometry.
/// Native count/role checks run in the AU loader; format acceptance runs during activation.
#[cfg(target_vendor = "apple")]
pub(super) fn selected_au_layout(selected: &ProviderPorts) -> Result<BusLayout, String> {
    selected.validate().map_err(|error| error.to_string())?;
    let bus = |port: &ProviderBus| Bus {
        name: port.name.clone(),
        kind: match port.kind {
            ProviderBusKind::Main => BusKind::Main,
            ProviderBusKind::Sidechain => BusKind::Sidechain,
            ProviderBusKind::Auxiliary => BusKind::Auxiliary,
        },
        channels: match port.channel_format {
            ProviderChannelFormat::Mono => ChannelConfig::Mono,
            ProviderChannelFormat::Stereo => ChannelConfig::Stereo,
            ProviderChannelFormat::Surround5_1 => ChannelConfig::Surround5_1,
            ProviderChannelFormat::Surround7_1 => ChannelConfig::Surround7_1,
            ProviderChannelFormat::Discrete => ChannelConfig::Discrete(u32::from(port.channels)),
        },
    };
    let layout = BusLayout {
        id: None,
        inputs: selected.inputs.iter().map(bus).collect(),
        outputs: selected.outputs.iter().map(bus).collect(),
    };
    if !same_topology(&describe(&layout)?, selected) {
        return Err("selected AU layout identity does not match its bus geometry".into());
    }
    Ok(layout)
}

pub(super) fn verify_active(
    plugin: &dyn truce_rack_core::PluginCore,
    selected: &ProviderPorts,
) -> Result<(), String> {
    let active = plugin
        .active_layout()
        .ok_or("processor did not publish an active bus configuration")?;
    if !plugin.is_active() || !same_topology(&describe(active)?, selected) {
        return Err("processor activated a different bus configuration".into());
    }
    Ok(())
}

pub(super) fn verified_layouts<P: truce_rack_core::Plugin<f32>>(
    plugin: &mut P,
) -> Result<Vec<ProviderPorts>, String> {
    let layouts = plugin.supported_layouts();
    let candidates = layouts
        .iter()
        .filter_map(|layout| {
            // Generators and note-only layouts are outside this audio-effect catalog.
            if layout.inputs.is_empty() || layout.outputs.is_empty() {
                return None;
            }
            Some(describe(layout).map(|ports| (layout.clone(), ports)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut verified = Vec::with_capacity(candidates.len());
    let mut last_rejection = None;
    for (layout, ports) in candidates {
        if verified
            .iter()
            .any(|previous: &ProviderPorts| previous.layout_id == ports.layout_id)
        {
            continue;
        }
        let result = plugin.activate(layout, 48_000.0, 128);
        if let Err(truce_rack_core::Error::ActivationRejected { reason }) = &result
            && !plugin.is_active()
        {
            tracing::debug!(layout = %ports.layout_id, %reason, "processor configuration rejected");
            last_rejection = Some(reason.clone());
            continue;
        }
        let result = result
            .map_err(|error| error.to_string())
            .and_then(|()| verify_active(plugin, &ports));
        if plugin.is_active() {
            plugin.deactivate();
        }
        if plugin.is_active() {
            return Err(format!(
                "processor layout {} did not deactivate",
                ports.layout_id
            ));
        }
        result.map_err(|error| format!("processor layout {} failed: {error}", ports.layout_id))?;
        verified.push(ports);
    }
    if verified.is_empty() {
        return Err(last_rejection.map_or_else(
            || "plugin exposes no supported audio effect layout".into(),
            |cause| format!("plugin exposes no usable audio effect layout: {cause}"),
        ));
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_vendor = "apple")]
    #[test]
    fn selected_au_round_trips_ordered_geometry_and_rejects_forged_identity() {
        let original = BusLayout::stereo_with_sidechain("Key");
        let mut ports = describe(&original).unwrap();
        assert_eq!(selected_au_layout(&ports).unwrap(), original);
        ports.layout_id.push('0');
        assert!(selected_au_layout(&ports).is_err());
        ports = describe(&original).unwrap();
        ports.inputs[1].id = PortId::from("wrong");
        assert!(selected_au_layout(&ports).is_err());
    }

    #[test]
    fn layout_identity_preserves_bus_order_and_native_id_not_display_text() {
        let mut layout = BusLayout::stereo_with_sidechain("Key input");
        layout.id = Some("native:4294967295".into());
        let original = describe(&layout).unwrap();
        layout.inputs[1].name = "Translated".into();
        assert!(same_topology(&original, &describe(&layout).unwrap()));
        layout.inputs.swap(0, 1);
        assert_ne!(original.layout_id, describe(&layout).unwrap().layout_id);
        layout.inputs.swap(0, 1);
        layout.id = Some("native:1".into());
        assert_ne!(original.layout_id, describe(&layout).unwrap().layout_id);
    }

    #[test]
    fn worker_rejects_same_total_with_different_bus_topology() {
        let sidechain = BusLayout::stereo_with_sidechain("Key");
        let selected = describe(&sidechain).unwrap();
        let mut flat = BusLayout::stereo();
        flat.inputs[0].channels = ChannelConfig::Discrete(4);
        assert!(native_layout(&[flat], &selected).is_err());
        assert!(native_layout(&[sidechain], &selected).is_ok());
    }

    #[test]
    fn topology_bounds_reject_instead_of_narrowing() {
        let mut layout = BusLayout::stereo();
        layout.inputs[0].channels = ChannelConfig::Discrete(64);
        assert_eq!(describe(&layout).unwrap().inputs[0].channels, 64);
        layout.inputs[0].channels = ChannelConfig::Discrete(65);
        assert!(describe(&layout).is_err());
        layout.inputs[0].channels = ChannelConfig::Discrete(u32::MAX);
        assert!(describe(&layout).is_err());
    }
}
