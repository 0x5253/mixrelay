//! Prepared signal identity, excluding controls and ordinary DSP parameters.
use std::{collections::BTreeSet, mem::Discriminant};

use mixrelay_audio_graph::{
    CompiledEdge, CompiledNode, CompiledPort, Effect, ExecutionPlan, NodeConfig, NodeId, NodeKind,
    NodeTypeId,
};
use mixrelay_audio_io::{ApplicationSelector, DeviceSelector, StreamSpec, VirtualEndpointId};

/// Comparing edge IDs alone misses a new device or application selected in an
/// existing node. Preserve histories only while their complete feeding signal
/// identity remains the same. All collection work happens during preparation.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::render) struct InputPath {
    routes: Box<[CompiledEdge]>,
    nodes: Box<[SignalNode]>,
}

#[derive(Debug, PartialEq, Eq)]
struct SignalNode {
    id: NodeId,
    kind: NodeKind,
    inputs: Box<[CompiledPort]>,
    outputs: Box<[CompiledPort]>,
    origin: Origin,
}

#[derive(Debug, PartialEq, Eq)]
enum Origin {
    Processing,
    Device(DeviceSelector),
    Application {
        selector: ApplicationSelector,
        stream: StreamSpec,
        requires_reselection: bool,
    },
    Virtual {
        id: VirtualEndpointId,
        render: Option<StreamSpec>,
        capture: Option<StreamSpec>,
        rate: u32,
    },
    Effect(Discriminant<Effect>),
    Provider(NodeTypeId),
}

impl InputPath {
    pub(in crate::render) fn prepare(execution: &ExecutionPlan, target: &NodeId) -> Self {
        let terminals: BTreeSet<_> = execution
            .nodes()
            .iter()
            .filter(|node| matches!(node.config(), NodeConfig::VirtualEndpointTransport(_)))
            .map(CompiledNode::id)
            .collect();
        let mut reached = BTreeSet::from([target.clone()]);
        let mut pending = vec![target.clone()];
        while let Some(node) = pending.pop() {
            // Driver playback contributions do not feed its independent capture
            // stream. Actual canonical feedback is represented by DeviceBus edges.
            if terminals.contains(&node) {
                continue;
            }
            for edge in execution
                .edges()
                .iter()
                .filter(|edge| edge.destination().node_id == node)
            {
                if reached.insert(edge.source().node_id.clone()) {
                    pending.push(edge.source().node_id.clone());
                }
            }
        }
        let mut routes: Vec<_> = execution
            .edges()
            .iter()
            .filter(|edge| {
                reached.contains(&edge.destination().node_id)
                    && !terminals.contains(&edge.destination().node_id)
            })
            .cloned()
            .collect();
        routes.sort_unstable_by(|left, right| left.id().cmp(right.id()));
        let mut nodes: Vec<_> = execution
            .nodes()
            .iter()
            .filter(|node| reached.contains(node.id()))
            .map(SignalNode::from)
            .collect();
        nodes.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        Self {
            routes: routes.into_boxed_slice(),
            nodes: nodes.into_boxed_slice(),
        }
    }
}

impl From<&CompiledNode> for SignalNode {
    fn from(node: &CompiledNode) -> Self {
        let origin = match node.config() {
            NodeConfig::AudioDeviceInput(config) => Origin::Device(config.selector.clone()),
            NodeConfig::ApplicationCapture(config) => Origin::Application {
                selector: config.selector.clone(),
                stream: config.stream,
                requires_reselection: config.requires_reselection,
            },
            NodeConfig::VirtualEndpoint(config) => virtual_origin(&config.spec),
            NodeConfig::VirtualEndpointTransport(spec) => virtual_origin(spec),
            NodeConfig::Effect(config) => Origin::Effect(std::mem::discriminant(&config.effect)),
            NodeConfig::Provider(config) => Origin::Provider(config.type_id.clone()),
            _ => Origin::Processing,
        };
        Self {
            id: node.id().clone(),
            kind: node.kind(),
            inputs: node.inputs().into(),
            outputs: node.outputs().into(),
            origin,
        }
    }
}

fn virtual_origin(spec: &mixrelay_audio_io::VirtualEndpointSpec) -> Origin {
    Origin::Virtual {
        id: spec.id.clone(),
        render: spec.render,
        capture: spec.capture,
        rate: spec.default_sample_rate,
    }
}
