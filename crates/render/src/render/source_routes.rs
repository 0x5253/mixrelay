//! Control-plane reachability excludes capture snapshots with no downstream listener.
use mixrelay_audio_graph::{
    CompiledEdge, ExecutionPlan, NodeConfig, NodeId, PortId, TO_OS_PORT_ID,
};
use std::collections::BTreeSet;

pub(super) fn reaches_sink(execution: &ExecutionPlan, source: &NodeId, port: &PortId) -> bool {
    let mut pending: Vec<_> = execution
        .edges()
        .iter()
        .filter(|edge| &edge.source().node_id == source && &edge.source().port_id == port)
        .map(CompiledEdge::destination)
        .collect();
    let mut visited = BTreeSet::new();
    while let Some(destination) = pending.pop() {
        let Some(node) = execution
            .nodes()
            .iter()
            .find(|node| node.id() == &destination.node_id)
        else {
            continue;
        };
        if matches!(
            node.config(),
            NodeConfig::AudioDeviceOutput(_) | NodeConfig::Recorder { .. }
        ) || (matches!(node.config(), NodeConfig::VirtualEndpointTransport(_))
            && destination.port_id.as_str() == TO_OS_PORT_ID)
        {
            return true;
        }
        // Device buses can close a time-boundary cycle. Visit each intermediate
        // node once; an input port alone never implies an external output path.
        if visited.insert(node.id()) {
            pending.extend(
                execution
                    .edges()
                    .iter()
                    .filter(|edge| &edge.source().node_id == node.id())
                    .map(CompiledEdge::destination),
            );
        }
    }
    false
}
