//! Native bus facts, checked activation, and bounded per-block bus views.
//! A channel count is not a speaker mask: unknown/discrete layouts preserve the
//! arrangement reported by the plugin instead of silently forcing stereo.

use sha2::{Digest, Sha256};
use std::ptr;
use truce_rack_core::{
    bus::{Bus, BusKind, BusLayout, ChannelConfig},
    error::{Error, Result},
};
use vst3::{
    ComPtr,
    Steinberg::{
        Vst::{
            BusInfo,
            BusInfo_::{BusFlags, BusFlags_},
            BusTypes, BusTypes_, IAudioProcessor, IAudioProcessorTrait, IComponent,
            IComponentTrait, SpeakerArr,
        },
        kResultOk,
    },
};

const MAX_CHANNELS: usize = 64;
const MAX_BUSES: usize = 64;
const AUDIO: i32 = 0;
const INPUT: i32 = 0;
const OUTPUT: i32 = 1;

fn invalid(message: &str) -> Error {
    Error::Other(format!("VST3 bus layout: {message}"))
}

fn flag_bits(value: BusFlags) -> u32 {
    u32::from_ne_bytes(value.to_ne_bytes())
}

fn bus_type_value(value: BusTypes) -> i32 {
    i32::from_ne_bytes(value.to_ne_bytes())
}

fn bus_count(component: &ComPtr<IComponent>, direction: i32) -> Result<usize> {
    // SAFETY: `component` is retained and `direction` is one of the fixed native audio directions.
    let count = usize::try_from(unsafe { component.getBusCount(AUDIO, direction) })
        .map_err(|_| invalid("negative bus count"))?;
    if count > MAX_BUSES {
        return Err(invalid("bus count exceeds host capacity"));
    }
    Ok(count)
}

fn bus_info(component: &ComPtr<IComponent>, direction: i32, index: usize) -> Result<BusInfo> {
    let mut info = BusInfo {
        mediaType: AUDIO,
        direction,
        channelCount: 0,
        name: [0; 128],
        busType: 0,
        flags: 0,
    };
    let index = i32::try_from(index).map_err(|_| invalid("bus index overflow"))?;
    // SAFETY: `info` is a live writable local, and `index` is bounded by `getBusCount` callers.
    if unsafe { component.getBusInfo(AUDIO, direction, index, &raw mut info) } != kResultOk {
        return Err(invalid("getBusInfo failed"));
    }
    if info.mediaType != AUDIO
        || info.direction != direction
        || !(0..=64).contains(&info.channelCount)
    {
        return Err(invalid("invalid native bus geometry"));
    }
    Ok(info)
}

fn arrangement(processor: &ComPtr<IAudioProcessor>, direction: i32, index: usize) -> Result<u64> {
    let mut mask = 0;
    let index = i32::try_from(index).map_err(|_| invalid("bus index overflow"))?;
    // SAFETY: `mask` is a live writable local; the retained processor receives an index from its
    // checked bus count and a fixed audio direction.
    if unsafe { processor.getBusArrangement(direction, index, &raw mut mask) } != kResultOk {
        return Err(invalid("getBusArrangement failed"));
    }
    Ok(mask)
}

#[derive(Clone)]
struct NativeBus {
    bus: Bus,
    facts: NativeBusFacts,
    default_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeBusFacts {
    direction: i32,
    index: usize,
    bus_type: i32,
    control_voltage: bool,
    channel_count: u32,
    arrangement: u64,
}

#[derive(Clone)]
struct NativeTopology {
    inputs: Vec<NativeBus>,
    outputs: Vec<NativeBus>,
}

/// One exact arrangement and active-bus prefix proven against the native instance.
#[derive(Clone, Debug)]
pub(super) struct NativeLayout {
    pub(super) layout: BusLayout,
    input_facts: Vec<NativeBusFacts>,
    output_facts: Vec<NativeBusFacts>,
    input_active_prefix: usize,
    output_active_prefix: usize,
}

fn bus_facts(
    info: &BusInfo,
    direction: i32,
    index: usize,
    arrangement: u64,
) -> Result<NativeBusFacts> {
    let channel_count =
        u32::try_from(info.channelCount).map_err(|_| invalid("negative channel count"))?;
    if arrangement.count_ones() != channel_count {
        return Err(invalid("channel count and speaker arrangement differ"));
    }
    Ok(NativeBusFacts {
        direction,
        index,
        bus_type: info.busType,
        control_voltage: info.flags & flag_bits(BusFlags_::kIsControlVoltage) != 0,
        channel_count,
        arrangement,
    })
}

fn read_bus_facts(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    direction: i32,
    index: usize,
) -> Result<NativeBusFacts> {
    let info = bus_info(component, direction, index)?;
    let mask = arrangement(processor, direction, index)?;
    bus_facts(&info, direction, index, mask)
}

fn read_buses(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    direction: i32,
) -> Result<Vec<NativeBus>> {
    let mut buses = Vec::new();
    for index in 0..bus_count(component, direction)? {
        let info = bus_info(component, direction, index)?;
        let mask = arrangement(processor, direction, index)?;
        let facts = bus_facts(&info, direction, index, mask)?;
        if facts.control_voltage {
            return Err(invalid(&format!(
                "unsupported control-voltage bus at direction {direction}, index {index}"
            )));
        }
        let kind = native_bus_kind(facts.bus_type, direction)?;
        let channels = match mask {
            SpeakerArr::kMono => ChannelConfig::Mono,
            SpeakerArr::kStereo => ChannelConfig::Stereo,
            SpeakerArr::k51 => ChannelConfig::Surround5_1,
            SpeakerArr::k71Music => ChannelConfig::Surround7_1,
            _ => ChannelConfig::Discrete(facts.channel_count),
        };
        buses.push(NativeBus {
            bus: Bus {
                name: crate::metadata::string128_to_string(&info.name),
                kind,
                channels,
            },
            facts,
            default_active: info.flags & flag_bits(BusFlags_::kDefaultActive) != 0,
        });
    }
    Ok(buses)
}

fn native_bus_kind(bus_type: i32, direction: i32) -> Result<BusKind> {
    match bus_type {
        value if value == bus_type_value(BusTypes_::kMain) => Ok(BusKind::Main),
        value if value == bus_type_value(BusTypes_::kAux) && direction == INPUT => {
            Ok(BusKind::Sidechain)
        }
        value if value == bus_type_value(BusTypes_::kAux) => Ok(BusKind::Auxiliary),
        _ => Err(invalid("unknown native audio bus type")),
    }
}

fn read_topology(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
) -> Result<NativeTopology> {
    Ok(NativeTopology {
        inputs: read_buses(component, processor, INPUT)?,
        outputs: read_buses(component, processor, OUTPUT)?,
    })
}

fn default_active_prefix(buses: &[NativeBus]) -> Option<usize> {
    let prefix = buses
        .iter()
        .position(|bus| !bus.default_active)
        .unwrap_or(buses.len());
    buses[prefix..]
        .iter()
        .all(|bus| !bus.default_active)
        .then_some(prefix)
}

fn update_length(digest: &mut Sha256, length: usize) -> Result<()> {
    let length = u64::try_from(length).map_err(|_| invalid("native topology length overflow"))?;
    digest.update(length.to_le_bytes());
    Ok(())
}

fn update_bus_facts(digest: &mut Sha256, facts: &[NativeBusFacts]) -> Result<()> {
    update_length(digest, facts.len())?;
    for fact in facts {
        digest.update(fact.direction.to_le_bytes());
        update_length(digest, fact.index)?;
        digest.update(fact.bus_type.to_le_bytes());
        digest.update([u8::from(fact.control_voltage)]);
        digest.update(fact.channel_count.to_le_bytes());
        digest.update(fact.arrangement.to_le_bytes());
    }
    Ok(())
}

fn native_layout_id(
    input_facts: &[NativeBusFacts],
    output_facts: &[NativeBusFacts],
    input_active_prefix: usize,
    output_active_prefix: usize,
) -> Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut digest = Sha256::new();
    digest.update(b"truce-rack-vst3-layout-v2\0");
    update_bus_facts(&mut digest, input_facts)?;
    update_bus_facts(&mut digest, output_facts)?;
    update_length(&mut digest, input_active_prefix)?;
    update_length(&mut digest, output_active_prefix)?;
    let mut id = String::with_capacity(8 + 64);
    id.push_str("vst3:v2:");
    for byte in digest.finalize() {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(id)
}

fn make_layout(
    topology: &NativeTopology,
    input_active_prefix: usize,
    output_active_prefix: usize,
) -> Result<NativeLayout> {
    if input_active_prefix > topology.inputs.len() || output_active_prefix > topology.outputs.len()
    {
        return Err(invalid("active bus prefix exceeds native topology"));
    }
    let input_facts = topology
        .inputs
        .iter()
        .map(|bus| bus.facts)
        .collect::<Vec<_>>();
    let output_facts = topology
        .outputs
        .iter()
        .map(|bus| bus.facts)
        .collect::<Vec<_>>();
    let id = native_layout_id(
        &input_facts,
        &output_facts,
        input_active_prefix,
        output_active_prefix,
    )?;
    Ok(NativeLayout {
        layout: BusLayout {
            id: Some(id),
            inputs: topology.inputs[..input_active_prefix]
                .iter()
                .map(|bus| bus.bus.clone())
                .collect(),
            outputs: topology.outputs[..output_active_prefix]
                .iter()
                .map(|bus| bus.bus.clone())
                .collect(),
        },
        input_facts,
        output_facts,
        input_active_prefix,
        output_active_prefix,
    })
}

fn set_arrangements(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    inputs: &[NativeBusFacts],
    outputs: &[NativeBusFacts],
) -> Result<()> {
    verify_native_facts(component, processor, inputs, outputs)?;
    let mut input_arrangements = inputs
        .iter()
        .map(|fact| fact.arrangement)
        .collect::<Vec<_>>();
    let mut output_arrangements = outputs
        .iter()
        .map(|fact| fact.arrangement)
        .collect::<Vec<_>>();
    let input_count =
        i32::try_from(input_arrangements.len()).map_err(|_| invalid("input bus count overflow"))?;
    let output_count = i32::try_from(output_arrangements.len())
        .map_err(|_| invalid("output bus count overflow"))?;
    let input_ptr = if input_arrangements.is_empty() {
        ptr::null_mut()
    } else {
        input_arrangements.as_mut_ptr()
    };
    let output_ptr = if output_arrangements.is_empty() {
        ptr::null_mut()
    } else {
        output_arrangements.as_mut_ptr()
    };
    // SAFETY: both arrays remain alive and writable for the synchronous native call; counts
    // exactly match the component's native audio bus counts, with null for an empty direction.
    if unsafe { processor.setBusArrangements(input_ptr, input_count, output_ptr, output_count) }
        != kResultOk
    {
        return Err(invalid(
            "setBusArrangements rejected exact native arrangements",
        ));
    }
    verify_native_facts(component, processor, inputs, outputs)
}

fn verify_direction_facts(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    direction: i32,
    expected: &[NativeBusFacts],
) -> Result<()> {
    if expected.len() != bus_count(component, direction)? {
        return Err(invalid("readback bus count differs from native topology"));
    }
    for (index, expected) in expected.iter().copied().enumerate() {
        if expected.direction != direction
            || expected.index != index
            || read_bus_facts(component, processor, direction, index)? != expected
        {
            return Err(invalid(
                "native bus facts differ from the discovered topology",
            ));
        }
    }
    Ok(())
}

fn verify_native_facts(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    inputs: &[NativeBusFacts],
    outputs: &[NativeBusFacts],
) -> Result<()> {
    verify_direction_facts(component, processor, INPUT, inputs)?;
    verify_direction_facts(component, processor, OUTPUT, outputs)
}

fn update_bus_states(
    component: &ComPtr<IComponent>,
    input_count: usize,
    output_count: usize,
    input_prefix: usize,
    output_prefix: usize,
) -> Result<()> {
    if input_prefix > input_count || output_prefix > output_count {
        return Err(invalid("requested active prefix exceeds native topology"));
    }
    let mut failed = false;
    for (direction, count, active_prefix) in [
        (INPUT, input_count, input_prefix),
        (OUTPUT, output_count, output_prefix),
    ] {
        for index in 0..count {
            let Ok(native_index) = i32::try_from(index) else {
                failed = true;
                continue;
            };
            let active = u8::from(index < active_prefix);
            // SAFETY: the component is retained and each index is within the checked native bus
            // count captured during discovery; these calls run on the serialized lifecycle thread.
            failed |= unsafe { component.activateBus(AUDIO, direction, native_index, active) }
                != kResultOk;
        }
    }
    if failed {
        return Err(invalid("activateBus failed while applying the bus prefix"));
    }
    Ok(())
}

pub(super) fn activate_buses(
    component: &ComPtr<IComponent>,
    layout: &NativeLayout,
    active: bool,
) -> Result<()> {
    let prefixes = if active {
        (layout.input_active_prefix, layout.output_active_prefix)
    } else {
        (0, 0)
    };
    let result = update_bus_states(
        component,
        layout.input_facts.len(),
        layout.output_facts.len(),
        prefixes.0,
        prefixes.1,
    );
    if result.is_err() && active {
        let cleanup = update_bus_states(
            component,
            layout.input_facts.len(),
            layout.output_facts.len(),
            0,
            0,
        );
        if cleanup.is_err() {
            return Err(invalid(
                "activateBus failed and all-bus cleanup also failed",
            ));
        }
    }
    result
}

fn restore_baseline(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    topology: &NativeTopology,
) -> Result<()> {
    let inputs = topology
        .inputs
        .iter()
        .map(|bus| bus.facts)
        .collect::<Vec<_>>();
    let outputs = topology
        .outputs
        .iter()
        .map(|bus| bus.facts)
        .collect::<Vec<_>>();
    let arrangements = set_arrangements(component, processor, &inputs, &outputs);
    let buses = update_bus_states(component, inputs.len(), outputs.len(), 0, 0);
    arrangements.and(buses)
}

fn probe_candidate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    topology: &NativeTopology,
    candidate: NativeLayout,
) -> Result<Option<NativeLayout>> {
    let probe = (|| {
        set_arrangements(
            component,
            processor,
            &candidate.input_facts,
            &candidate.output_facts,
        )?;
        activate_buses(component, &candidate, true)?;
        activate_buses(component, &candidate, false)
    })();
    restore_baseline(component, processor, topology)?;
    Ok(probe.is_ok().then_some(candidate))
}

fn active_channels_fit(buses: &[Bus]) -> bool {
    buses
        .iter()
        .map(|bus| u64::from(bus.channels.count()))
        .sum::<u64>()
        <= MAX_CHANNELS as u64
}

fn candidate_prefixes(topology: &NativeTopology) -> Vec<(usize, usize)> {
    let full_prefix = (topology.inputs.len(), topology.outputs.len());
    let mut candidates = vec![full_prefix];
    if let (Some(input_prefix), Some(output_prefix)) = (
        default_active_prefix(&topology.inputs),
        default_active_prefix(&topology.outputs),
    ) {
        let default_prefix = (input_prefix, output_prefix);
        if default_prefix != full_prefix {
            candidates.push(default_prefix);
        }
        // Native counts are bounded by MAX_BUSES. Keep full/default identities first,
        // then probe intermediate combinations of the declared trailing optional buses.
        // A hole in the default flags is not a contiguous layout and is never guessed.
        candidates.extend(
            (input_prefix..=full_prefix.0)
                .flat_map(|inputs| {
                    (output_prefix.max(1)..=full_prefix.1).map(move |outputs| (inputs, outputs))
                })
                .filter(|prefix| *prefix != full_prefix && *prefix != default_prefix),
        );
    }
    candidates
}

pub(super) fn discover(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
) -> Result<Vec<NativeLayout>> {
    let topology = read_topology(component, processor)?;
    let candidates = candidate_prefixes(&topology);
    let mut layouts = Vec::with_capacity(candidates.len());
    for (input_prefix, output_prefix) in candidates {
        let candidate = make_layout(&topology, input_prefix, output_prefix)?;
        // Inactive native buses still bind layout identity but consume no PCM channel slots.
        if !active_channels_fit(&candidate.layout.inputs)
            || !active_channels_fit(&candidate.layout.outputs)
        {
            continue;
        }
        if let Some(candidate) = probe_candidate(component, processor, &topology, candidate)? {
            layouts.push(candidate);
        }
    }
    if layouts.is_empty() {
        return Err(invalid(
            "no exact native bus topology passed activation probing",
        ));
    }
    Ok(layouts)
}

pub(super) fn select_layout(
    layouts: &[NativeLayout],
    requested: &BusLayout,
) -> Result<NativeLayout> {
    layouts
        .iter()
        .find(|candidate| &candidate.layout == requested)
        .cloned()
        .ok_or_else(|| invalid("selected layout was not advertised by this instance"))
}

pub(super) fn negotiate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    layout: &NativeLayout,
) -> Result<()> {
    validate_public_prefix(
        &layout.layout.inputs,
        &layout.input_facts,
        layout.input_active_prefix,
        INPUT,
    )?;
    validate_public_prefix(
        &layout.layout.outputs,
        &layout.output_facts,
        layout.output_active_prefix,
        OUTPUT,
    )?;
    set_arrangements(
        component,
        processor,
        &layout.input_facts,
        &layout.output_facts,
    )
}

fn validate_public_prefix(
    buses: &[Bus],
    facts: &[NativeBusFacts],
    prefix: usize,
    direction: i32,
) -> Result<()> {
    if buses.len() != prefix || prefix > facts.len() {
        return Err(invalid(
            "selected public topology differs from native bus prefixes",
        ));
    }
    if !active_channels_fit(buses) {
        return Err(invalid("active channel count exceeds host capacity"));
    }
    for (index, (bus, fact)) in buses.iter().zip(facts).enumerate() {
        if fact.direction != direction
            || fact.index != index
            || fact.control_voltage
            || bus.channels.count() != fact.channel_count
            || bus.kind != native_bus_kind(fact.bus_type, direction)?
        {
            return Err(invalid(
                "selected public buses differ from native bus facts",
            ));
        }
    }
    Ok(())
}

mod buffers;
pub(super) use buffers::with_buffers;

#[cfg(test)]
mod tests;
