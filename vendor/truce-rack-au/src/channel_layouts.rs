use objc2_audio_toolbox::{AUChannelInfo, kAudioUnitProperty_SupportedNumChannels};

use crate::bus_topology::{discover_bus_layout, validate_bus_channels};
use crate::stream_format::{
    apply_and_verify_native_formats, capture_native_formats, restore_native_formats,
};
use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnitGetProperty, AudioUnitGetPropertyInfo, kAudioUnitScope_Global,
};
use std::ptr;
use truce_rack_core::{
    bus::{BusLayout, ChannelConfig},
    error::{Error, Result},
};

type CatalogTopology = (Vec<BusLayout>, Vec<u32>, Vec<u32>);

const MAX_CHANNELS: u32 = 64;
const MAX_CAPABILITIES: usize = 4096;

#[cfg(test)]
mod native_tests;

fn matches_count(declared: i16, channels: u32) -> bool {
    match declared {
        -2 | -1 => channels > 0,
        limit if limit < -2 => channels > 0 && channels <= u32::from(limit.unsigned_abs()),
        exact => u32::try_from(exact).ok() == Some(channels),
    }
}

fn candidate_pairs(capabilities: &[AUChannelInfo], has_input: bool) -> Vec<(u32, u32)> {
    // Some AUv2 effects (including Apple's limiter) omit this optional property. These are
    // only proposals: every width must still pass native format negotiation and readback.
    if capabilities.is_empty() {
        return (1..=MAX_CHANNELS)
            .map(|output| (if has_input { output } else { 0 }, output))
            .collect();
    }
    let mut pairs = Vec::new();
    let inputs = if has_input { 1..=MAX_CHANNELS } else { 0..=0 };
    for input in inputs {
        for output in 1..=MAX_CHANNELS {
            if capabilities
                .iter()
                .any(|entry| matches_pair(*entry, (input, output)))
            {
                pairs.push((input, output));
            }
        }
    }
    pairs
}

fn matches_pair(entry: AUChannelInfo, pair: (u32, u32)) -> bool {
    matches_count(entry.inChannels, pair.0)
        && matches_count(entry.outChannels, pair.1)
        && (entry.inChannels != -1 || entry.outChannels != -1 || pair.0 == pair.1)
}

fn same_bus_roles(left: &BusLayout, right: &BusLayout) -> bool {
    left.id == right.id
        && left.inputs.len() == right.inputs.len()
        && left.outputs.len() == right.outputs.len()
        && left
            .inputs
            .iter()
            .zip(&right.inputs)
            .all(|(a, b)| a.kind == b.kind)
        && left
            .outputs
            .iter()
            .zip(&right.outputs)
            .all(|(a, b)| a.kind == b.kind)
}

/// Retain only the requested layout for a worker; do not mutate or probe alternative formats.
///
/// # Safety
/// The new, inactive AU is exclusively owned by `AuPlugin::load_from`, which disposes it on
/// error. Activation must negotiate the selected format and initialize before processing.
pub(super) unsafe fn select(
    unit: AudioComponentInstance,
    selected: &BusLayout,
) -> Result<CatalogTopology> {
    let inputs: Vec<_> = selected
        .inputs
        .iter()
        .map(|bus| bus.channels.count())
        .collect();
    let outputs: Vec<_> = selected
        .outputs
        .iter()
        .map(|bus| bus.channels.count())
        .collect();
    validate_bus_channels(&inputs, &outputs)?;
    // SAFETY: serialized reads of the exclusively owned, inactive instance.
    let (original, _, _) = unsafe { discover_bus_layout(unit)? };
    if !same_bus_roles(&original, selected) {
        return Err(Error::Other(
            "selected AU bus identity, order or roles changed".into(),
        ));
    }
    if inputs.len() <= 1 && outputs.len() == 1 {
        let declarations = unsafe { capabilities(unit)? };
        let pair = (
            selected.total_input_channels(),
            selected.total_output_channels(),
        );
        if !declarations.is_empty() && !declarations.iter().any(|entry| matches_pair(*entry, pair))
        {
            return Err(Error::Other(
                "selected AU channel counts are not declared supported".into(),
            ));
        }
    }
    Ok((vec![selected.clone()], inputs, outputs))
}

/// Read the optional capability array, with bounded storage and exact native byte counts.
///
/// # Safety
/// The live, inactive instance is exclusively owned by `AuPlugin::load_from`.
unsafe fn capabilities(unit: AudioComponentInstance) -> Result<Vec<AUChannelInfo>> {
    let mut size = 0;
    // SAFETY: load owns the instance; size is a writable UInt32 and no writable flag is needed.
    let status = unsafe {
        AudioUnitGetPropertyInfo(
            unit,
            kAudioUnitProperty_SupportedNumChannels,
            kAudioUnitScope_Global,
            0,
            &raw mut size,
            ptr::null_mut(),
        )
    };
    if status == objc2_audio_toolbox::kAudioUnitErr_InvalidProperty {
        return Ok(Vec::new());
    }
    let bytes = usize::try_from(size).unwrap_or(usize::MAX);
    let item_size = std::mem::size_of::<AUChannelInfo>();
    if status != 0 || !bytes.is_multiple_of(item_size) || bytes / item_size > MAX_CAPABILITIES {
        return Err(Error::Other(format!(
            "invalid AU channel capabilities: status {status}, size {size}"
        )));
    }
    if bytes == 0 {
        return Ok(Vec::new());
    }
    let mut entries = vec![
        AUChannelInfo {
            inChannels: 0,
            outChannels: 0
        };
        bytes / item_size
    ];
    let mut returned = size;
    // SAFETY: entries is initialized, aligned AUChannelInfo storage of exactly size bytes.
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_SupportedNumChannels,
            kAudioUnitScope_Global,
            0,
            ptr::NonNull::new_unchecked(entries.as_mut_ptr().cast()),
            ptr::NonNull::from(&mut returned),
        )
    };
    if status != 0 || returned != size {
        return Err(Error::Other(format!(
            "AU channel capabilities read failed: status {status}, size {returned}/{size}"
        )));
    }
    Ok(entries)
}

/// Try a channel pair; native readback supplies names, grouping and roles, not the declaration.
///
/// # Safety
/// `proposed` has the observed single-bus topology of this live, inactive instance. The caller
/// restores its captured formats after every attempt, including partially rejected changes.
unsafe fn probe_pair(
    unit: AudioComponentInstance,
    proposed: &mut BusLayout,
    pair: (u32, u32),
) -> Option<BusLayout> {
    for bus in &mut proposed.inputs {
        bus.channels = ChannelConfig::Discrete(pair.0);
    }
    for bus in &mut proposed.outputs {
        bus.channels = ChannelConfig::Discrete(pair.1);
    }
    let input = [pair.0];
    let inputs = if proposed.inputs.is_empty() {
        &[][..]
    } else {
        &input[..]
    };
    // SAFETY: load exclusively owns the inactive instance; discovery always restores its
    // snapshot. Reuse activation's negotiation at the catalog probe's existing 48 kHz rate.
    unsafe { apply_and_verify_native_formats(unit, inputs, &[pair.1], proposed, 48_000.0) }.ok()?;
    // SAFETY: native queries are serialized with the format writes above.
    let (layout, _, _) = unsafe { discover_bus_layout(unit) }.ok()?;
    let expected_inputs = usize::from(pair.0 > 0);
    (layout.inputs.len() == expected_inputs
        && layout.outputs.len() == 1
        && layout.total_input_channels() == pair.0
        && layout.total_output_channels() == pair.1)
        .then_some(layout)
}

/// Enumerate channel-width alternatives without changing the AU's original native topology.
/// Multi-bus units retain their observed topology: the global channel declaration does not
/// specify how to distribute a total across individual auxiliary elements.
///
/// # Safety
/// The instance is newly created and exclusively owned by `AuPlugin::load_from`, which disposes
/// it if discovery or restoration fails. No render callback or listener is installed yet.
pub(super) unsafe fn discover(unit: AudioComponentInstance) -> Result<CatalogTopology> {
    // SAFETY: the owning load path supplies a live, uninitialized instance exclusively.
    let (original, inputs, outputs) = unsafe { discover_bus_layout(unit)? };
    let mut layouts = vec![original.clone()];
    if inputs.len() > 1 || outputs.len() != 1 {
        return Ok((layouts, inputs, outputs));
    }
    // SAFETY: all queries and temporary writes below occur on the serialized load path.
    let declarations = unsafe { capabilities(unit)? };
    let previous = unsafe { capture_native_formats(unit, &inputs, &outputs)? };
    let mut proposed = original.clone();
    for pair in candidate_pairs(&declarations, !inputs.is_empty()) {
        if pair
            == (
                original.total_input_channels(),
                original.total_output_channels(),
            )
        {
            continue;
        }
        // SAFETY: every attempt is followed by checked restoration, even if rejected.
        let candidate = unsafe { probe_pair(unit, &mut proposed, pair) };
        let failures = unsafe { restore_native_formats(unit, &previous) };
        if !failures.is_empty() {
            return Err(Error::Other(format!(
                "AU layout discovery rollback failed: {}",
                failures.join("; ")
            )));
        }
        // SAFETY: still exclusively owned; compare semantic topology as well as raw formats.
        if unsafe { discover_bus_layout(unit)? }.0 != original {
            return Err(Error::Other(
                "AU layout discovery changed the original topology".into(),
            ));
        }
        if let Some(layout) = candidate {
            // Candidate pairs are unique and readback confirmed both counts, so layouts
            // cannot duplicate each other. The original pair was excluded above.
            layouts.push(layout);
        }
    }
    Ok((layouts, inputs, outputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(input: i16, output: i16) -> AUChannelInfo {
        AUChannelInfo {
            inChannels: input,
            outChannels: output,
        }
    }

    #[test]
    fn equal_wildcards_preserve_pairing_and_channel_limit() {
        let pairs = candidate_pairs(&[capability(-1, -1)], true);
        assert_eq!(pairs, (1..=64).map(|n| (n, n)).collect::<Vec<_>>());
    }

    #[test]
    fn independent_wildcards_include_asymmetric_widths() {
        let pairs = candidate_pairs(&[capability(-1, -2)], true);
        assert_eq!(pairs.len(), 4096);
        assert!(pairs.contains(&(32, 16)));
        assert_eq!(pairs, candidate_pairs(&[capability(-2, -1)], true));
    }

    #[test]
    fn fixed_bounded_and_duplicate_capabilities_are_exact() {
        assert_eq!(
            candidate_pairs(&[capability(2, 6), capability(2, 6)], true),
            vec![(2, 6)]
        );
        assert_eq!(
            candidate_pairs(&[capability(-3, 1)], true),
            vec![(1, 1), (2, 1), (3, 1)]
        );
        assert!(candidate_pairs(&[capability(65, 65)], true).is_empty());
        assert_eq!(
            candidate_pairs(&[], true),
            candidate_pairs(&[capability(-1, -1)], true)
        );
        assert_eq!(
            candidate_pairs(&[], false),
            candidate_pairs(&[capability(0, -1)], false)
        );
    }

    #[test]
    fn generators_do_not_gain_an_input_and_effects_do_not_lose_one() {
        assert_eq!(candidate_pairs(&[capability(0, 2)], false), vec![(0, 2)]);
        assert!(candidate_pairs(&[capability(-1, -1)], false).is_empty());
        assert!(candidate_pairs(&[capability(0, 2)], true).is_empty());
        assert!(candidate_pairs(&[capability(2, 0)], true).is_empty());
    }
}
