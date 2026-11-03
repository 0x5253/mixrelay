//! Hosted provider layout validation and control-thread processor preparation.
use super::{LoweredKernels, LoweringContext, LoweringState, node_slot};
use crate::render::{
    DryBusMap, HostedIo, HostedProcessorPrepareRequest, Kernel, PortKey, PortSlot, RenderPlanError,
};
use mixrelay_audio_graph::{
    CompiledNode, MAX_PROVIDER_BUSES_PER_DIRECTION, MAX_PROVIDER_CHANNELS_PER_DIRECTION,
    PortDirection, ProviderBus, ProviderBusKind, ProviderGeometryError, ProviderNodeConfig,
    ProviderPorts, validate_provider_geometry,
};
use std::collections::BTreeMap;

struct HostedProcessorIo {
    io: HostedIo,
    input_channels: u16,
    output_channels: u16,
}

pub(super) fn lower_hosted_processor(
    node: &CompiledNode,
    config: &ProviderNodeConfig,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    let Some(factory) = context.hosted_factory else {
        if !node.control().enabled {
            return Ok(LoweredKernels::default());
        }
        return Err(RenderPlanError::UnsupportedProvider {
            node_id: node.id().clone(),
        });
    };
    let prepared_io = prepare_hosted_processor_io(node, config, context, state)?;
    let request = HostedProcessorPrepareRequest {
        node_id: node.id(),
        config,
        format: context.format,
        input_channels: prepared_io.input_channels,
        output_channels: prepared_io.output_channels,
    };
    let processor = if node.control().enabled {
        factory.prepare(request).map(Some)
    } else {
        factory.prepare_inactive(request)
    }
    .map_err(|reason| RenderPlanError::ProviderPreparation {
        node_id: node.id().clone(),
        reason,
    })?;
    let Some(processor) = processor else {
        return Ok(LoweredKernels::default());
    };
    let processor_index = state.hosted_processors.len();
    state.hosted_processors.push(processor);
    Ok(LoweredKernels {
        sources: vec![],
        processing: vec![Kernel::Hosted {
            io: prepared_io.io,
            processor: processor_index,
        }],
        sinks: vec![],
    })
}

fn prepare_hosted_processor_io(
    node: &CompiledNode,
    config: &ProviderNodeConfig,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<HostedProcessorIo, RenderPlanError> {
    let inputs = provider_port_slots(node, PortDirection::Input, context.slots)?;
    let outputs = provider_port_slots(node, PortDirection::Output, context.slots)?;
    let input_channels = provider_total_channels(node, "input", &inputs)?;
    let output_channels = provider_total_channels(node, "output", &outputs)?;
    let saved_layout = provider_selected_layout(node, config)?;
    let single_main_bus = inputs.len() == 1
        && outputs.len() == 1
        && saved_layout.is_none_or(|layout| {
            layout.inputs[0].kind == ProviderBusKind::Main
                && layout.outputs[0].kind == ProviderBusKind::Main
        });
    let io = if single_main_bus {
        HostedIo::Single {
            input: inputs[0],
            output: outputs[0],
        }
    } else {
        let layout = saved_layout.ok_or_else(|| RenderPlanError::MissingProviderLayout {
            node_id: node.id().clone(),
        })?;
        let block_frames = context.format.block_frames as usize;
        let input_samples = block_frames
            .checked_mul(usize::from(input_channels))
            .ok_or(RenderPlanError::BufferSizeOverflow)?;
        let output_samples = block_frames
            .checked_mul(usize::from(output_channels))
            .ok_or(RenderPlanError::BufferSizeOverflow)?;
        *state.hosted_input_scratch_samples =
            (*state.hosted_input_scratch_samples).max(input_samples);
        *state.hosted_output_scratch_samples =
            (*state.hosted_output_scratch_samples).max(output_samples);
        HostedIo::Multiple {
            inputs: inputs.into_boxed_slice(),
            outputs: outputs.into_boxed_slice(),
            input_channels,
            output_channels,
            dry_maps: make_dry_bus_maps(layout),
        }
    };
    Ok(HostedProcessorIo {
        io,
        input_channels,
        output_channels,
    })
}

fn provider_selected_layout<'a>(
    node: &CompiledNode,
    config: &'a ProviderNodeConfig,
) -> Result<Option<&'a ProviderPorts>, RenderPlanError> {
    let Some(layout) = config.ports.as_ref() else {
        return Ok(None);
    };
    layout
        .validate()
        .map_err(|error| RenderPlanError::InvalidProviderLayout {
            node_id: node.id().clone(),
            reason: error.to_string(),
        })?;
    if !provider_bus_list_matches(&layout.inputs, node.inputs())
        || !provider_bus_list_matches(&layout.outputs, node.outputs())
    {
        return Err(RenderPlanError::ProviderLayoutMismatch {
            node_id: node.id().clone(),
        });
    }
    Ok(Some(layout))
}

fn provider_bus_list_matches(
    buses: &[ProviderBus],
    ports: &[mixrelay_audio_graph::CompiledPort],
) -> bool {
    buses.len() == ports.len()
        && buses
            .iter()
            .zip(ports)
            .all(|(bus, port)| bus.id == *port.id() && bus.channels == port.channels())
}

fn make_dry_bus_maps(layout: &ProviderPorts) -> Box<[DryBusMap]> {
    let mut main_inputs = Vec::new();
    let mut input_offset = 0;
    for bus in &layout.inputs {
        if bus.kind == ProviderBusKind::Main {
            main_inputs.push((input_offset, bus.channels));
        }
        input_offset += usize::from(bus.channels);
    }
    let mut mappings = Vec::new();
    let mut main_ordinal = 0;
    let mut output_offset = 0;
    for bus in &layout.outputs {
        if bus.kind == ProviderBusKind::Main {
            if let Some(&(source_channel_offset, input_channels)) = main_inputs.get(main_ordinal) {
                let channels = input_channels.min(bus.channels);
                if channels > 0 {
                    mappings.push(DryBusMap {
                        source_channel_offset,
                        destination_channel_offset: output_offset,
                        channels,
                    });
                }
            }
            main_ordinal += 1;
        }
        output_offset += usize::from(bus.channels);
    }
    mappings.into_boxed_slice()
}

fn provider_port_slots(
    node: &CompiledNode,
    direction: PortDirection,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<Vec<PortSlot>, RenderPlanError> {
    node_ports(node, direction)
        .iter()
        .map(|port| node_slot(node, port.id(), direction, slots))
        .collect()
}

fn node_ports(
    node: &CompiledNode,
    direction: PortDirection,
) -> &[mixrelay_audio_graph::CompiledPort] {
    match direction {
        PortDirection::Input => node.inputs(),
        PortDirection::Output => node.outputs(),
    }
}

fn provider_total_channels(
    node: &CompiledNode,
    direction: &'static str,
    ports: &[PortSlot],
) -> Result<u16, RenderPlanError> {
    validate_provider_geometry(ports.iter().map(|port| port.channels)).map_err(
        |error| match error {
            ProviderGeometryError::NoBuses | ProviderGeometryError::TooManyBuses { .. } => {
                RenderPlanError::InvalidProviderBusCount {
                    node_id: node.id().clone(),
                    direction,
                    maximum: MAX_PROVIDER_BUSES_PER_DIRECTION,
                    found: ports.len(),
                }
            }
            ProviderGeometryError::InvalidBusChannels { channels, .. } => {
                RenderPlanError::InvalidProviderBusChannels {
                    node_id: node.id().clone(),
                    direction,
                    channels,
                }
            }
            ProviderGeometryError::TooManyTotalChannels { channels } => {
                RenderPlanError::InvalidProviderTotalChannels {
                    node_id: node.id().clone(),
                    direction,
                    maximum: MAX_PROVIDER_CHANNELS_PER_DIRECTION,
                    channels,
                }
            }
        },
    )
}
