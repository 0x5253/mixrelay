//! Projects prepared-worker latency observations through the active execution's identities.

use std::collections::BTreeMap;

use crate::processor_ports::ProcessorLatencySnapshot;
use mixrelay_audio_graph::NodeId;
use mixrelay_control_protocol::ProcessorLatency;

use crate::{EngineGraphController, graph_composition::NodeOrigin};

impl EngineGraphController {
    pub(crate) fn processor_latencies(&self) -> Vec<ProcessorLatency> {
        let revision = self.exchange.plan_revisions().active;
        let views = self.execution_views.read();
        let Some(view) = views.get(&revision) else {
            return Vec::new();
        };
        project_latencies(&view.origins, |runtime_id| {
            self.processor_controls.latency_snapshot(runtime_id)
        })
    }
}

fn project_latencies(
    origins: &BTreeMap<NodeId, NodeOrigin>,
    snapshot: impl Fn(&NodeId) -> Option<ProcessorLatencySnapshot>,
) -> Vec<ProcessorLatency> {
    origins
        .iter()
        .filter_map(|(runtime_id, origin)| {
            snapshot(runtime_id).map(|latency| ProcessorLatency {
                graph_id: origin.graph_id.clone(),
                node_id: origin.node_id.as_str().to_owned(),
                transport_frames: latency.transport_frames,
                intrinsic_frames: latency.intrinsic_frames,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_same_named_nodes_per_graph_and_omits_unavailable_workers() {
        let origins = ["graph-a", "graph-b", "graph-c"]
            .into_iter()
            .map(|graph| {
                (
                    NodeId::new(format!("{graph}/effect")),
                    NodeOrigin {
                        graph_id: graph.to_owned(),
                        node_id: NodeId::from("effect"),
                        meter_port: None,
                    },
                )
            })
            .collect();
        let observations = project_latencies(&origins, |runtime_id| match runtime_id.as_str() {
            "graph-a/effect" => Some(ProcessorLatencySnapshot {
                transport_frames: 32,
                intrinsic_frames: None,
            }),
            "graph-b/effect" => Some(ProcessorLatencySnapshot {
                transport_frames: 256,
                intrinsic_frames: Some(0),
            }),
            _ => None,
        });
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].graph_id, "graph-a");
        assert_eq!(observations[1].graph_id, "graph-b");
        assert_eq!(observations[0].node_id, "effect");
        assert_eq!(observations[1].node_id, "effect");
        assert_eq!(observations[0].transport_frames, 32);
        assert_eq!(observations[1].transport_frames, 256);
        assert_eq!(observations[0].intrinsic_frames, None);
        assert_eq!(observations[1].intrinsic_frames, Some(0));
        assert!(project_latencies(&origins, |_| None).is_empty());
    }
}
