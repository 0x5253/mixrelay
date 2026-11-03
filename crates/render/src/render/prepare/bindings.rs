//! Prepares external, meter, recorder, history and control bindings.
use super::node_slot;

use super::super::{
    DeviceBusBinding, DynamicsBinding, DynamicsMeterBinding, ExternalBinding, MeterBinding,
    PortKey, PortSlot, RenderPlanError, RuntimeControlBank, effects, history, sample_count,
    source_routes,
};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, CompiledNode, ExecutionPlan, FROM_OS_PORT_ID, NodeConfig, NodeId, PortDirection,
    PortId, TO_OS_PORT_ID,
};
use std::collections::BTreeMap;

pub(super) fn device_bus_bindings(
    execution: &ExecutionPlan,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<Box<[DeviceBusBinding]>, RenderPlanError> {
    let mut offset = 0;
    let mut bindings = Vec::new();
    for (control_index, node) in execution.nodes().iter().enumerate() {
        let NodeConfig::DeviceBus { .. } = node.config() else {
            continue;
        };
        let input = node_slot(
            node,
            &PortId::from(AUDIO_PORT_ID),
            PortDirection::Input,
            slots,
        )?;
        // Called only after lowering has checked the same capacities for overflow.
        let end = offset + sample_count(execution.format().block_frames as usize, input.channels);
        bindings.push(DeviceBusBinding {
            node_id: node.id().clone(),
            samples: offset..end,
            control_index,
            input_path: history::InputPath::prepare(execution, node.id()),
        });
        offset = end;
    }
    bindings.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    Ok(bindings.into_boxed_slice())
}

pub(super) fn dynamics_bindings(execution: &ExecutionPlan) -> Box<[DynamicsBinding]> {
    // State indices follow lowering order; only the common history registry is sorted.
    let bindings: Vec<_> = execution
        .nodes()
        .iter()
        .filter(|node| {
            matches!(
                node.config(),
                NodeConfig::Compressor(_) | NodeConfig::Limiter(_)
            )
        })
        .enumerate()
        .map(|(state_index, node)| DynamicsBinding {
            node_id: node.id().clone(),
            config: node.config().clone(),
            state_index,
        })
        .collect();
    bindings.into_boxed_slice()
}

pub(super) fn prepare_effects(
    execution: &ExecutionPlan,
) -> Result<Box<[effects::Binding]>, RenderPlanError> {
    let mut bindings = Vec::new();
    let mut bytes = 0_usize;
    for node in execution.nodes() {
        if let NodeConfig::Effect(config) = node.config() {
            let design = effects::Design::new(config, execution.format())
                .map_err(RenderPlanError::BuiltinPreparation)?;
            bytes = bytes
                .checked_add(
                    design
                        .storage_bytes()
                        .map_err(RenderPlanError::BuiltinPreparation)?,
                )
                .ok_or(RenderPlanError::BufferSizeOverflow)?;
            if bytes > effects::MAX_EFFECT_BYTES {
                return Err(RenderPlanError::BuiltinPreparation(
                    "aggregate built-in effect memory capacity exceeded".into(),
                ));
            }
            bindings.push(effects::Binding {
                node_id: node.id().clone(),
                design,
            });
        }
    }
    Ok(bindings.into_boxed_slice())
}

pub(super) fn binding_controls<'a>(
    controls: &RuntimeControlBank,
    ids: impl Iterator<Item = &'a NodeId>,
) -> Result<Box<[usize]>, RenderPlanError> {
    ids.map(|id| {
        controls
            .index(id)
            .ok_or_else(|| RenderPlanError::UncompiledBinding {
                node_id: id.clone(),
            })
    })
    .collect()
}

pub(super) fn collect_external_bindings(
    execution: &ExecutionPlan,
    slots: &BTreeMap<PortKey, PortSlot>,
    direction: PortDirection,
) -> Result<Vec<ExternalBinding>, RenderPlanError> {
    let mut bindings = Vec::new();
    for node in execution.nodes() {
        if matches!(node.config(), NodeConfig::VirtualEndpoint(_)) {
            return Err(RenderPlanError::UnloweredVirtualEndpoint {
                node_id: node.id().clone(),
            });
        }
        let port_id = match (node.config(), direction) {
            (
                NodeConfig::ApplicationCapture(_) | NodeConfig::AudioDeviceInput(_),
                PortDirection::Output,
            )
            | (NodeConfig::AudioDeviceOutput(_), PortDirection::Input) => Some(AUDIO_PORT_ID),
            (NodeConfig::VirtualEndpointTransport(config), PortDirection::Output)
                if config.capture.is_some() =>
            {
                Some(FROM_OS_PORT_ID)
            }
            (NodeConfig::VirtualEndpointTransport(config), PortDirection::Input)
                if config.render.is_some() =>
            {
                Some(TO_OS_PORT_ID)
            }
            _ => None,
        };
        if let Some(port_id) = port_id {
            let port_id = PortId::from(port_id);
            // Sources must keep being drained and metered before a cable is
            // attached. Only an unconnected external sink may be omitted.
            if direction == PortDirection::Input
                && !matches!(node.config(), NodeConfig::VirtualEndpointTransport(_))
                && !external_port_is_connected(execution, node.id(), &port_id, direction)
            {
                continue;
            }
            let slot = node_slot(node, &port_id, direction, slots)?;
            bindings.push(ExternalBinding {
                node_id: node.id().clone(),
                kind: node.kind(),
                routed_source: direction == PortDirection::Output
                    && source_routes::reaches_sink(execution, node.id(), &port_id),
                port_id,
                channels: slot.channels,
                virtual_endpoint_id: match node.config() {
                    NodeConfig::VirtualEndpointTransport(endpoint) => Some(endpoint.id.clone()),
                    _ => None,
                },
            });
        }
    }
    bindings.sort_unstable_by(|left, right| {
        left.node_id
            .cmp(&right.node_id)
            .then(left.port_id.cmp(&right.port_id))
    });
    Ok(bindings)
}

pub(super) fn collect_meter_bindings(
    execution: &ExecutionPlan,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<Vec<MeterBinding>, RenderPlanError> {
    let mut bindings = Vec::with_capacity(execution.nodes().len());
    let mut dynamics_index = 0;
    for node in execution.nodes() {
        let threshold = match node.config() {
            NodeConfig::Compressor(config) => Some(config.threshold_db),
            NodeConfig::Limiter(config) => Some(config.ceiling_db),
            _ => None,
        };
        let dynamics = if let Some(threshold_db) = threshold {
            let binding = DynamicsMeterBinding {
                input: node_slot(
                    node,
                    &PortId::from(AUDIO_PORT_ID),
                    PortDirection::Input,
                    slots,
                )?,
                state_index: dynamics_index,
                threshold_db,
            };
            dynamics_index += 1;
            Some(binding)
        } else {
            None
        };
        for port in node.outputs() {
            let mut binding = meter_binding(node, port.id(), PortDirection::Output, slots)?;
            binding.dynamics = dynamics;
            bindings.push(binding);
        }
        if node.outputs().is_empty()
            || matches!(node.config(), NodeConfig::VirtualEndpointTransport(_))
        {
            for port in node.inputs() {
                bindings.push(meter_binding(node, port.id(), PortDirection::Input, slots)?);
            }
        }
    }
    Ok(bindings)
}

pub(super) fn collect_recorder_bindings(
    execution: &ExecutionPlan,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<Vec<MeterBinding>, RenderPlanError> {
    execution
        .nodes()
        .iter()
        .filter(|node| matches!(node.config(), NodeConfig::Recorder { .. }))
        .map(|node| {
            meter_binding(
                node,
                &PortId::from(AUDIO_PORT_ID),
                PortDirection::Input,
                slots,
            )
        })
        .collect()
}

fn meter_binding(
    node: &CompiledNode,
    port_id: &PortId,
    direction: PortDirection,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<MeterBinding, RenderPlanError> {
    let slot = node_slot(node, port_id, direction, slots)?;
    Ok(MeterBinding {
        node_id: node.id().clone(),
        port_id: port_id.clone(),
        channels: slot.channels,
        slot,
        dynamics: None,
    })
}

fn external_port_is_connected(
    execution: &ExecutionPlan,
    node_id: &NodeId,
    port_id: &PortId,
    direction: PortDirection,
) -> bool {
    execution.edges().iter().any(|edge| {
        let endpoint = match direction {
            PortDirection::Input => edge.destination(),
            PortDirection::Output => edge.source(),
        };
        &endpoint.node_id == node_id && &endpoint.port_id == port_id
    })
}
