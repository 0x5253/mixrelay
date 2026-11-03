//! Shared built-in history policy: retain a compatible feeding timeline, reset a new one.

use std::{collections::BTreeMap, ops::Range};

use mixrelay_audio_graph::{ExecutionPlan, NodeConfig};

use super::{BlockProcessor, DeviceBusBinding, DynamicsBinding, ExternalBinding, effects};

mod path;
pub(super) use path::InputPath;

#[derive(Debug, Clone, Copy)]
pub(super) enum State {
    Effect(usize),
    Dynamics(usize),
}

impl State {
    pub(super) fn reset(self, effects: &mut [effects::State], envelopes: &mut [f32]) {
        match self {
            Self::Effect(index) => effects[index].suspend(),
            Self::Dynamics(index) => envelopes[index] = 0.0,
        }
    }
}

#[derive(Debug)]
pub(super) struct Binding {
    node_id: mixrelay_audio_graph::NodeId,
    input_path: InputPath,
    pub(super) state: State,
}

pub(super) fn bindings(
    execution: &ExecutionPlan,
    effects: &[effects::Binding],
    dynamics: &[DynamicsBinding],
) -> Box<[Binding]> {
    let mut bindings: Vec<_> = effects
        .iter()
        .enumerate()
        .map(|(index, binding)| (&binding.node_id, State::Effect(index)))
        .chain(
            dynamics
                .iter()
                .map(|binding| (&binding.node_id, State::Dynamics(binding.state_index))),
        )
        .map(|(node_id, state)| Binding {
            node_id: node_id.clone(),
            input_path: InputPath::prepare(execution, node_id),
            state,
        })
        .collect();
    bindings.sort_unstable_by(|left, right| left.node_id.cmp(&right.node_id));
    bindings.into_boxed_slice()
}

pub(super) fn retain(current: &mut BlockProcessor, previous: &mut BlockProcessor) {
    if current.plan.format != previous.plan.format {
        return;
    }
    for (index, binding) in current.plan.source_bindings.iter().enumerate() {
        if let Some(previous_index) = previous
            .plan
            .source_bindings
            .iter()
            .position(|old| old == binding)
        {
            current.source_timelines[index] = previous.source_timelines[previous_index];
        }
    }
    for binding in &current.plan.history {
        let Ok(index) = previous
            .plan
            .history
            .binary_search_by(|candidate| candidate.node_id.cmp(&binding.node_id))
        else {
            continue;
        };
        let old = &previous.plan.history[index];
        if binding.input_path != old.input_path {
            continue;
        }
        match (binding.state, old.state) {
            (State::Effect(index), State::Effect(old_index)) => {
                if current.plan.effects[index]
                    .design
                    .compatible(&previous.plan.effects[old_index].design)
                {
                    std::mem::swap(
                        &mut current.effect_states[index],
                        &mut previous.effect_states[old_index],
                    );
                }
            }
            (State::Dynamics(index), State::Dynamics(old_index)) => {
                let compatible = match (
                    &current.plan.dynamics_bindings[index].config,
                    &previous.plan.dynamics_bindings[old_index].config,
                ) {
                    (NodeConfig::Compressor(new), NodeConfig::Compressor(old)) => {
                        new.version == old.version && new.channels == old.channels
                    }
                    (NodeConfig::Limiter(new), NodeConfig::Limiter(old)) => {
                        new.version == old.version && new.channels == old.channels
                    }
                    _ => false,
                };
                if compatible {
                    current.dynamics_envelopes[index] = previous.dynamics_envelopes[old_index];
                    current.dynamics_wet[index] = previous.dynamics_wet[old_index];
                }
            }
            _ => {}
        }
    }
}

/// Only histories reachable from the restarted input belong to its invalidation scope.
#[derive(Debug)]
pub(super) struct Targets {
    pub(super) device_buses: Box<[Range<usize>]>,
    pub(super) delay_routes: Box<[usize]>,
    pub(super) histories: Box<[State]>,
}

/// Walks the compiled topology off the audio thread, retaining only reset indices.
/// A visited set bounds causal feedback traversal; no graph walk occurs during rendering.
pub(super) fn prepare(
    execution: &ExecutionPlan,
    sources: &[ExternalBinding],
    buses: &[DeviceBusBinding],
    histories: &[Binding],
) -> Box<[Targets]> {
    let indices: BTreeMap<_, _> = execution
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id(), index))
        .collect();
    let routes: Vec<_> = execution
        .edges()
        .iter()
        .map(|edge| {
            (
                indices[&edge.source().node_id],
                indices[&edge.destination().node_id],
            )
        })
        .collect();
    let mut outgoing = vec![Vec::new(); indices.len()];
    for &(source, destination) in &routes {
        outgoing[source].push(destination);
    }
    let mut reached = vec![false; indices.len()];
    let mut pending = Vec::with_capacity(indices.len());
    sources
        .iter()
        .map(|source| {
            reached.fill(false);
            let start = indices[&source.node_id];
            reached[start] = true;
            pending.push(start);
            while let Some(node) = pending.pop() {
                for &next in &outgoing[node] {
                    // A transport's graph-to-OS sink does not feed its independent
                    // OS-to-graph source. Canonical mixing is represented by real bus edges.
                    let terminal = matches!(
                        execution.nodes()[next].config(),
                        NodeConfig::VirtualEndpointTransport(_)
                    );
                    if !reached[next] && !terminal {
                        reached[next] = true;
                        pending.push(next);
                    }
                }
            }
            Targets {
                histories: histories
                    .iter()
                    .filter_map(|binding| {
                        reached[indices[&binding.node_id]].then_some(binding.state)
                    })
                    .collect(),
                device_buses: buses
                    .iter()
                    .filter(|bus| reached[indices[&bus.node_id]])
                    .map(|bus| bus.samples.clone())
                    .collect(),
                delay_routes: routes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &(from, _))| reached[from].then_some(index))
                    .collect(),
            }
        })
        .collect()
}
