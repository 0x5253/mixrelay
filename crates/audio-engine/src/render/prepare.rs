//! Control-thread compilation from a validated graph into immutable render operations.
use super::{
    HostedProcessorFactory, LoweredKernels, LoweredRoutes, NodeStep, PortKey, PortSlot,
    PreparedRenderPlan, RenderPlan, RenderPlanError, RenderSchedule, RouteOp, RuntimeControlBank,
    allocate_slots, compensation, history,
};
use mixrelay_audio_graph::{
    AudioEndpoint, CompiledNode, ExecutionPlan, NodeConfig, PortDirection, PortId,
};
use std::{collections::BTreeMap, sync::Arc};

mod bindings;
mod lowering;
mod providers;
use bindings::{
    binding_controls, collect_external_bindings, collect_meter_bindings, collect_recorder_bindings,
    device_bus_bindings, dynamics_bindings, prepare_effects,
};
use lowering::{LoweringContext, LoweringState, lower_kernels};

#[allow(clippy::too_many_lines)] // Ordered control-thread preparation of independent bounded resources.
pub(super) fn plan(
    execution: &ExecutionPlan,
    hosted_factory: Option<&dyn HostedProcessorFactory>,
) -> Result<PreparedRenderPlan, RenderPlanError> {
    let block_frames = execution.format().block_frames as usize;
    let (slots, arena_samples) = allocate_slots(execution.nodes(), block_frames)?;
    let mut routes = lower_routes(execution, &slots);
    let source_bindings = collect_external_bindings(execution, &slots, PortDirection::Output)?;
    let sink_bindings = collect_external_bindings(execution, &slots, PortDirection::Input)?;
    let meter_bindings = collect_meter_bindings(execution, &slots)?;
    let recorder_bindings = collect_recorder_bindings(execution, &slots)?;
    let mut steps = Vec::with_capacity(execution.nodes().len());
    let mut prepared = Vec::with_capacity(execution.nodes().len());
    let mut dynamics_state_count = 0;
    let mut effect_count = 0;
    let effects = prepare_effects(execution)?;
    let mut device_bus_samples = 0;
    let mut hosted_input_scratch_samples = 0;
    let mut hosted_output_scratch_samples = 0;
    let mut hosted_processors = Vec::new();

    let context = LoweringContext {
        slots: &slots,
        source_bindings: &source_bindings,
        sink_bindings: &sink_bindings,
        format: execution.format(),
        hosted_factory,
    };

    for node in execution.nodes() {
        let incoming_routes = routes.incoming.remove(node.id()).unwrap_or_default();
        let mut state = LoweringState {
            dynamics_state_count: &mut dynamics_state_count,
            effect_count: &mut effect_count,
            device_bus_samples: &mut device_bus_samples,
            hosted_input_scratch_samples: &mut hosted_input_scratch_samples,
            hosted_output_scratch_samples: &mut hosted_output_scratch_samples,
            hosted_processors: &mut hosted_processors,
        };
        let kernels = lower_kernels(node, &context, &mut state)?;
        prepared.push(
            !matches!(node.config(), NodeConfig::Provider(_)) || !kernels.processing.is_empty(),
        );
        steps.push(NodeStep::new(node, kernels, incoming_routes, &slots)?);
    }

    let controls = Arc::new(RuntimeControlBank::new(execution, &prepared));
    let source_controls = binding_controls(&controls, source_bindings.iter().map(|b| &b.node_id))?;
    let sink_controls = binding_controls(&controls, sink_bindings.iter().map(|b| &b.node_id))?;
    let recorder_controls =
        binding_controls(&controls, recorder_bindings.iter().map(|b| &b.node_id))?;
    let device_bus_bindings = device_bus_bindings(execution, &slots)?;
    let dynamics_bindings = dynamics_bindings(execution);
    let history = history::bindings(execution, &effects, &dynamics_bindings);
    let source_history =
        history::prepare(execution, &source_bindings, &device_bus_bindings, &history);
    let hosted_scratch_samples = hosted_input_scratch_samples
        .checked_add(hosted_output_scratch_samples)
        .ok_or(RenderPlanError::BufferSizeOverflow)?;

    Ok((
        RenderPlan {
            revision: execution.revision(),
            format: execution.format(),
            schedule: RenderSchedule::new(&steps),
            compensation: compensation::CompensationTopology::new(
                execution,
                &steps,
                &hosted_processors,
            ),
            steps: steps.into_boxed_slice(),
            sink_routes: routes.sinks.into_boxed_slice(),
            recorder_bindings: recorder_bindings.into_boxed_slice(),
            source_bindings: source_bindings.into_boxed_slice(),
            sink_bindings: sink_bindings.into_boxed_slice(),
            meter_bindings: meter_bindings.into_boxed_slice(),
            arena_samples,
            arena_slots: slots.values().copied().collect(),
            dynamics_state_count,
            dynamics_bindings,
            effects,
            device_bus_samples,
            device_bus_bindings,
            history,
            source_history,
            hosted_input_scratch_samples,
            hosted_scratch_samples,
            controls,
            source_controls,
            sink_controls,
            recorder_controls,
        },
        hosted_processors.into_boxed_slice(),
    ))
}

impl NodeStep {
    fn new(
        node: &CompiledNode,
        kernels: LoweredKernels,
        incoming_routes: Vec<RouteOp>,
        slots: &BTreeMap<PortKey, PortSlot>,
    ) -> Result<Self, RenderPlanError> {
        let ports = |direction| {
            let ports = match direction {
                PortDirection::Input => node.inputs(),
                PortDirection::Output => node.outputs(),
            };
            ports
                .iter()
                .map(|port| node_slot(node, port.id(), direction, slots))
                .collect::<Result<Box<[_]>, _>>()
        };
        Ok(Self {
            incoming_routes: incoming_routes.into_boxed_slice(),
            sources: kernels.sources.into_boxed_slice(),
            processing: kernels.processing.into_boxed_slice(),
            sinks: kernels.sinks.into_boxed_slice(),
            inputs: ports(PortDirection::Input)?,
            outputs: ports(PortDirection::Output)?,
        })
    }
}

fn lower_routes(execution: &ExecutionPlan, slots: &BTreeMap<PortKey, PortSlot>) -> LoweredRoutes {
    let mut lowered = LoweredRoutes::default();
    let controls: BTreeMap<_, _> = execution
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id(), index))
        .collect();
    for (route_index, edge) in execution.edges().iter().enumerate() {
        let source_control = controls[&edge.source().node_id];
        let destination_control = controls[&edge.destination().node_id];
        let source = compiled_edge_slot(slots, edge.source(), PortDirection::Output);
        let destination = compiled_edge_slot(slots, edge.destination(), PortDirection::Input);
        let channels = edge
            .routes()
            .iter()
            .map(|route| (route.source_channel, route.destination_channel));
        let route = RouteOp::new(
            source,
            destination,
            [source_control, destination_control],
            channels,
            route_index,
        );
        if is_external_sink(execution, edge.destination()) {
            lowered.sinks.push(route);
            continue;
        }
        lowered
            .incoming
            .entry(edge.destination().node_id.clone())
            .or_default()
            .push(route);
    }
    lowered
}

fn is_external_sink(execution: &ExecutionPlan, endpoint: &AudioEndpoint) -> bool {
    execution.nodes().iter().any(|node| {
        node.id() == &endpoint.node_id
            && matches!(
                node.config(),
                NodeConfig::AudioDeviceOutput(_)
                    | NodeConfig::Recorder { .. }
                    | NodeConfig::VirtualEndpointTransport(_)
                    | NodeConfig::DeviceBus { .. }
            )
    })
}

fn compiled_edge_slot(
    slots: &BTreeMap<PortKey, PortSlot>,
    endpoint: &AudioEndpoint,
    direction: PortDirection,
) -> PortSlot {
    slots[&PortKey {
        endpoint: endpoint.clone(),
        direction,
    }]
}

fn node_slot(
    node: &CompiledNode,
    port_id: &PortId,
    direction: PortDirection,
    slots: &BTreeMap<PortKey, PortSlot>,
) -> Result<PortSlot, RenderPlanError> {
    required_slot(
        slots,
        &AudioEndpoint {
            node_id: node.id().clone(),
            port_id: port_id.clone(),
        },
        direction,
    )
}

fn required_slot(
    slots: &BTreeMap<PortKey, PortSlot>,
    endpoint: &AudioEndpoint,
    direction: PortDirection,
) -> Result<PortSlot, RenderPlanError> {
    slots
        .get(&PortKey {
            endpoint: endpoint.clone(),
            direction,
        })
        .copied()
        .ok_or_else(|| RenderPlanError::MissingPort {
            node_id: endpoint.node_id.clone(),
            port_id: endpoint.port_id.clone(),
        })
}
