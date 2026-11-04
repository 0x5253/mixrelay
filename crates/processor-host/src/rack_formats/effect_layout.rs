//! Channel bounds and the default choice among a processor's verified effect layouts.

use std::collections::HashSet;

use mixrelay_audio_graph::ProviderPorts;
use truce_rack_core::bus::{Bus, BusLayout};

use super::layouts;

const MAX_CHANNELS: u16 = 64;
/// Inline channel-reference capacity for one processed block.
pub(super) const CHANNEL_SLOTS: usize = MAX_CHANNELS as usize;

/// Total input and output channels, each within `1..=64`.
pub(super) fn layout_channels(layout: &BusLayout) -> Result<(u16, u16), String> {
    Ok((
        total_channels(&layout.inputs)?,
        total_channels(&layout.outputs)?,
    ))
}

fn total_channels(buses: &[Bus]) -> Result<u16, String> {
    let channels = buses
        .iter()
        .try_fold(0_u32, |total, bus| total.checked_add(bus.channels.count()));
    checked_channels(channels.unwrap_or(u32::MAX))
}

fn checked_channels(channels: u32) -> Result<u16, String> {
    u16::try_from(channels)
        .ok()
        .filter(|channels| (1..=MAX_CHANNELS).contains(channels))
        .ok_or_else(|| format!("plugin effect layout requires 1..={MAX_CHANNELS} channels"))
}

/// Prefers plain stereo, then symmetric layouts, then the fewest channels; the first of
/// equally ranked layouts wins. Layouts outside the channel bounds are never chosen.
pub(super) fn select_effect_layout(layouts: &[BusLayout]) -> Option<&BusLayout> {
    layouts
        .iter()
        .filter(|layout| layout_channels(layout).is_ok())
        .min_by_key(|layout| {
            let inputs = layout.total_input_channels();
            let outputs = layout.total_output_channels();
            let stereo_penalty = u32::from(inputs != 2 || outputs != 2);
            let asymmetric_penalty = u32::from(inputs != outputs);
            (stereo_penalty, asymmetric_penalty, inputs + outputs)
        })
}

/// The preferred native layout among those the probe activated successfully.
pub(super) fn default_verified_layout(
    supported: &[BusLayout],
    verified: &[ProviderPorts],
) -> Result<BusLayout, String> {
    let verified_ids: HashSet<&str> = verified
        .iter()
        .map(|ports| ports.layout_id.as_str())
        .collect();
    let usable: Vec<_> = supported
        .iter()
        .filter(|layout| {
            layouts::describe(layout)
                .is_ok_and(|ports| verified_ids.contains(ports.layout_id.as_str()))
        })
        .cloned()
        .collect();
    select_effect_layout(&usable)
        .cloned()
        .ok_or_else(|| "plugin exposes no supported audio effect layout".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use truce_rack_core::bus::ChannelConfig;

    fn discrete(inputs: u32, outputs: u32) -> BusLayout {
        let mut layout = BusLayout::new();
        layout
            .inputs
            .push(Bus::main("Input", ChannelConfig::Discrete(inputs)));
        layout
            .outputs
            .push(Bus::main("Output", ChannelConfig::Discrete(outputs)));
        layout
    }

    #[test]
    fn effect_layout_prefers_plain_stereo() {
        let layouts = [
            BusLayout::stereo_with_sidechain("Sidechain"),
            BusLayout::mono(),
            BusLayout::stereo(),
        ];

        let selected = select_effect_layout(&layouts).expect("one layout is usable");

        assert_eq!(selected, &layouts[2]);
    }

    #[test]
    fn effect_layout_without_stereo_prefers_symmetric_then_fewest_channels() {
        let layouts = [
            discrete(1, 2),
            discrete(4, 4),
            discrete(1, 1),
            discrete(2, 1),
        ];

        assert_eq!(select_effect_layout(&layouts), Some(&layouts[2]));
        assert_eq!(select_effect_layout(&layouts[..2]), Some(&layouts[1]));
    }

    #[test]
    fn effect_layout_rejects_empty_and_over_limit_layouts() {
        assert!(select_effect_layout(&[BusLayout::new(), discrete(65, 65)]).is_none());
    }

    #[test]
    fn default_layout_ignores_preferred_layouts_that_failed_verification() {
        let supported = [BusLayout::stereo(), BusLayout::mono()];
        let verified = [layouts::describe(&BusLayout::mono()).unwrap()];

        assert_eq!(
            default_verified_layout(&supported, &verified),
            Ok(BusLayout::mono())
        );
        assert!(default_verified_layout(&supported, &[]).is_err());
    }
}
