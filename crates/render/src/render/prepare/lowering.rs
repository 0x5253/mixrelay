//! Lowers node configurations into prepared render kernels.

use super::super::{
    DeviceBusKernel, ExternalBinding, HostedAudioProcessor, HostedProcessorFactory, Kernel,
    LoweredKernels, PortKey, PortSlot, RenderPlanError, sample_count,
};
use super::node_slot;
use super::providers::lower_hosted_processor;
use crate::dsp::db_to_linear;
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, CompiledNode, FROM_OS_PORT_ID, NodeConfig, NodeId, PortDirection, PortId,
    RenderFormat, TO_OS_PORT_ID,
};
use std::collections::BTreeMap;

pub(super) struct LoweringContext<'a> {
    pub(super) slots: &'a BTreeMap<PortKey, PortSlot>,
    pub(super) source_bindings: &'a [ExternalBinding],
    pub(super) sink_bindings: &'a [ExternalBinding],
    pub(super) format: RenderFormat,
    pub(super) hosted_factory: Option<&'a dyn HostedProcessorFactory>,
}

pub(super) struct LoweringState<'a> {
    pub(super) dynamics_state_count: &'a mut usize,
    pub(super) effect_count: &'a mut usize,
    pub(super) device_bus_samples: &'a mut usize,
    pub(super) hosted_input_scratch_samples: &'a mut usize,
    pub(super) hosted_output_scratch_samples: &'a mut usize,
    pub(super) hosted_processors: &'a mut Vec<Box<dyn HostedAudioProcessor>>,
}

pub(super) fn lower_kernels(
    node: &CompiledNode,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    match node.config() {
        NodeConfig::ApplicationCapture(_) | NodeConfig::AudioDeviceInput(_) => {
            lower_source_node(node, context.slots, context.source_bindings)
        }
        NodeConfig::AudioDeviceOutput(_) => Ok(LoweredKernels {
            sources: vec![],
            processing: vec![],
            sinks: sink_kernel(node, AUDIO_PORT_ID, context.slots, context.sink_bindings)?
                .into_iter()
                .collect(),
        }),
        NodeConfig::Recorder { .. } => Ok(LoweredKernels {
            sources: vec![],
            processing: vec![],
            sinks: vec![],
        }),
        NodeConfig::Compressor(_)
        | NodeConfig::Effect(_)
        | NodeConfig::Limiter(_)
        | NodeConfig::Mixer(_)
        | NodeConfig::Gain(_) => lower_builtin_processor(node, context, state),
        NodeConfig::DeviceBus { .. } => lower_device_bus(node, context, state),
        NodeConfig::VirtualEndpointTransport(config) => {
            lower_virtual_endpoint(node, config, context)
        }
        NodeConfig::VirtualEndpoint(_) => Err(RenderPlanError::UnloweredVirtualEndpoint {
            node_id: node.id().clone(),
        }),
        NodeConfig::Provider(config) => lower_hosted_processor(node, config, context, state),
    }
}

fn lower_effect(
    node: &CompiledNode,
    config: &mixrelay_audio_graph::EffectConfig,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    let (input, output) = processor_slots(node, context.slots)?;
    if input.channels != config.channels || output.channels != config.output_channels() {
        return Err(RenderPlanError::BuiltinPreparation(
            "effect port geometry does not match configuration".into(),
        ));
    }
    let sidechain = if matches!(config.effect, mixrelay_audio_graph::Effect::Ducking(_)) {
        Some(node_slot(
            node,
            &PortId::from(mixrelay_audio_graph::SIDECHAIN_PORT_ID),
            PortDirection::Input,
            context.slots,
        )?)
    } else {
        None
    };
    if sidechain.is_some_and(|port| port.channels != 1) {
        return Err(RenderPlanError::BuiltinPreparation(
            "ducking control input must be mono".into(),
        ));
    }
    let index = *state.effect_count;
    *state.effect_count += 1;
    Ok(LoweredKernels {
        sources: vec![],
        processing: vec![Kernel::Effect {
            input,
            output,
            sidechain,
            state: index,
        }],
        sinks: vec![],
    })
}

fn lower_builtin_processor(
    node: &CompiledNode,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    match node.config() {
        NodeConfig::Effect(config) => lower_effect(node, config, context, state),
        NodeConfig::Compressor(config) => lower_compressor(node, config, context, state),
        NodeConfig::Limiter(config) => {
            let (input, output) = processor_slots(node, context.slots)?;
            let state_index = *state.dynamics_state_count;
            *state.dynamics_state_count += 1;
            Ok(LoweredKernels {
                sources: vec![],
                processing: vec![Kernel::Limiter {
                    input,
                    output,
                    state_index,
                    ceiling_linear: db_to_linear(config.ceiling_db),
                    release_coefficient: smoothing_coefficient(
                        config.release_ms,
                        context.format.sample_rate,
                    ),
                    bypass: config.bypass,
                    bypass_coefficient: 1.0
                        - smoothing_coefficient(5.0, context.format.sample_rate),
                }],
                sinks: vec![],
            })
        }
        NodeConfig::Mixer(config) => {
            let inputs = config
                .inputs
                .iter()
                .map(|port_id| node_slot(node, port_id, PortDirection::Input, context.slots))
                .collect::<Result<Vec<_>, _>>()?
                .into_boxed_slice();
            let output = node_slot(
                node,
                &PortId::from(AUDIO_PORT_ID),
                PortDirection::Output,
                context.slots,
            )?;
            Ok(LoweredKernels {
                sources: vec![],
                processing: vec![Kernel::Mixer { inputs, output }],
                sinks: vec![],
            })
        }
        NodeConfig::Gain(_) => Ok(LoweredKernels {
            sources: vec![],
            processing: vec![Kernel::Gain {
                input: node_slot(
                    node,
                    &PortId::from(AUDIO_PORT_ID),
                    PortDirection::Input,
                    context.slots,
                )?,
                output: node_slot(
                    node,
                    &PortId::from(AUDIO_PORT_ID),
                    PortDirection::Output,
                    context.slots,
                )?,
            }],
            sinks: vec![],
        }),
        _ => unreachable!("non-built-in node passed to built-in lowering"),
    }
}

fn lower_compressor(
    node: &CompiledNode,
    config: &mixrelay_audio_graph::CompressorConfig,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    let (input, output) = processor_slots(node, context.slots)?;
    let state_index = *state.dynamics_state_count;
    *state.dynamics_state_count += 1;
    Ok(LoweredKernels {
        sources: vec![],
        processing: vec![Kernel::Compressor {
            input,
            output,
            state_index,
            threshold_linear: db_to_linear(config.threshold_db),
            exponent: 1.0 - 1.0 / config.ratio,
            attack_coefficient: smoothing_coefficient(config.attack_ms, context.format.sample_rate),
            release_coefficient: smoothing_coefficient(
                config.release_ms,
                context.format.sample_rate,
            ),
            makeup_gain: db_to_linear(config.makeup_gain_db),
            bypass: config.bypass,
            bypass_coefficient: 1.0 - smoothing_coefficient(5.0, context.format.sample_rate),
        }],
        sinks: vec![],
    })
}

/// Device buses retain one prepared quantum of PCM, regardless of callback partition or topology.
fn lower_device_bus(
    node: &CompiledNode,
    context: &LoweringContext<'_>,
    state: &mut LoweringState<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    let (input, output) = processor_slots(node, context.slots)?;
    let capacity = sample_count(context.format.block_frames as usize, input.channels);
    let snapshot = PortSlot {
        offset: *state.device_bus_samples,
        channels: input.channels,
    };
    *state.device_bus_samples = state
        .device_bus_samples
        .checked_add(capacity)
        .ok_or(RenderPlanError::BufferSizeOverflow)?;
    Ok(LoweredKernels {
        sources: vec![Kernel::DeviceBus(DeviceBusKernel::Read {
            snapshot,
            output,
            capacity,
        })],
        processing: vec![],
        sinks: vec![Kernel::DeviceBus(DeviceBusKernel::Publish {
            snapshot,
            input,
            capacity,
        })],
    })
}

fn lower_virtual_endpoint(
    node: &CompiledNode,
    config: &mixrelay_audio_graph::VirtualEndpointSpec,
    context: &LoweringContext<'_>,
) -> Result<LoweredKernels, RenderPlanError> {
    let mut kernels = LoweredKernels::default();
    if config.capture.is_some() {
        kernels.sources.extend(source_kernel(
            node,
            FROM_OS_PORT_ID,
            context.slots,
            context.source_bindings,
        )?);
    }
    if config.render.is_some() {
        kernels.sinks.extend(sink_kernel(
            node,
            TO_OS_PORT_ID,
            context.slots,
            context.sink_bindings,
        )?);
    }
    Ok(kernels)
}

fn processor_slots(
    node: &CompiledNode,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<(PortSlot, PortSlot), RenderPlanError> {
    let port = PortId::from(AUDIO_PORT_ID);
    Ok((
        node_slot(node, &port, PortDirection::Input, slots)?,
        node_slot(node, &port, PortDirection::Output, slots)?,
    ))
}

// Render-format validation caps the sample rate far below f32's exact-integer boundary.
#[allow(clippy::cast_precision_loss)]
fn smoothing_coefficient(milliseconds: f32, sample_rate: u32) -> f32 {
    crate::dsp::smoothing_coefficient(milliseconds * 0.001 * sample_rate as f32)
}

fn lower_source_node(
    node: &CompiledNode,
    slots: &BTreeMap<PortKey, PortSlot>,
    source_bindings: &[ExternalBinding],
) -> Result<LoweredKernels, RenderPlanError> {
    Ok(LoweredKernels {
        sources: source_kernel(node, AUDIO_PORT_ID, slots, source_bindings)?
            .into_iter()
            .collect(),
        processing: vec![],
        sinks: vec![],
    })
}

fn source_kernel(
    node: &CompiledNode,
    port_id: &str,
    slots: &BTreeMap<PortKey, PortSlot>,
    bindings: &[ExternalBinding],
) -> Result<Option<Kernel>, RenderPlanError> {
    let port_id = PortId::from(port_id);
    let output = node_slot(node, &port_id, PortDirection::Output, slots)?;
    let Some(binding) = binding_index(bindings, node.id(), &port_id) else {
        return Ok(None);
    };
    Ok(Some(Kernel::Source { binding, output }))
}

fn sink_kernel(
    node: &CompiledNode,
    port_id: &str,
    slots: &BTreeMap<PortKey, PortSlot>,
    bindings: &[ExternalBinding],
) -> Result<Option<Kernel>, RenderPlanError> {
    let port_id = PortId::from(port_id);
    let input = node_slot(node, &port_id, PortDirection::Input, slots)?;
    let Some(binding) = binding_index(bindings, node.id(), &port_id) else {
        return Ok(None);
    };
    Ok(Some(Kernel::Sink { binding, input }))
}

fn binding_index(
    bindings: &[ExternalBinding],
    node_id: &NodeId,
    port_id: &PortId,
) -> Option<usize> {
    bindings
        .iter()
        .position(|binding| binding.node_id() == node_id && binding.port_id() == port_id)
}
