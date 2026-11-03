//! Immutable authored control views, published only at committed writer boundaries.
//! Readers never acquire the graph mutation lock or perform native preparation.

use mixrelay_audio_graph::{GraphDocument, NodeConfig, NodeSpec};
use mixrelay_control_protocol::{
    EngineControlSnapshot, GraphControlSnapshot, GraphLayout, GraphNodeControls,
    NodeControlCapabilities, NodeControlDescriptor, NodeControlSnapshot, ProtocolError,
};

use crate::{EngineGraphController, graph_catalog::unavailable, node_control};

#[derive(Clone)]
pub(super) struct ControlReadView {
    snapshot: EngineControlSnapshot,
    recorder_ids: std::collections::BTreeMap<(String, String), String>,
}

impl EngineGraphController {
    pub(super) fn snapshot_recorder_status(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        let id = self
            .control_snapshot
            .read()
            .as_ref()
            .and_then(|view| {
                view.recorder_ids
                    .get(&(graph_id.to_owned(), node_id.to_owned()))
                    .cloned()
            })
            .ok_or_else(|| {
                node_control::node_error(
                    mixrelay_control_protocol::ErrorCode::NodeNotFound,
                    graph_id,
                    node_id,
                )
            })?;
        Ok(self.recorders.status(&id))
    }

    pub(super) fn refresh_control_snapshot(&self) {
        let graph = self.graph.read();
        let (graphs, recorder_ids) = self.catalog.inspect(|catalog| {
            let empty_layout = GraphLayout::default();
            let documents = catalog.and_then(|value| value.as_ref().ok()).map_or_else(
                || vec![("session", "Untitled Graph", &*graph, &empty_layout)],
                |value| value.control_documents().collect(),
            );
            let graphs = documents
                .iter()
                .map(|(id, name, document, layout)| graph_snapshot(id, name, document, layout))
                .collect();
            let recorder_ids = documents
                .iter()
                .flat_map(|(id, _, document, _)| {
                    document.nodes.iter().filter_map(move |node| {
                        let NodeConfig::Recorder { channels } = node.config else {
                            return None;
                        };
                        Some((
                            ((*id).to_owned(), node.id.as_str().to_owned()),
                            mixrelay_render::recording_session_key(
                                &crate::graph_composition::runtime_node_id(id, &node.id),
                                channels,
                            ),
                        ))
                    })
                })
                .collect();
            (graphs, recorder_ids)
        });
        let snapshot = EngineControlSnapshot {
            revision: graph.revision,
            active_revision: self.exchange.revisions().active,
            structure_generation: self.exchange.plan_revisions().submitted,
            engine_power: self.engine_power_status(),
            graphs,
        };
        drop(graph);
        *self.control_snapshot.write() = Some(ControlReadView {
            snapshot,
            recorder_ids,
        });
    }

    pub(super) fn snapshot_controls(&self) -> Result<EngineControlSnapshot, ProtocolError> {
        let ControlReadView {
            mut snapshot,
            recorder_ids,
        } = self
            .control_snapshot
            .read()
            .clone()
            .ok_or_else(|| unavailable("control snapshot is not initialized"))?;
        snapshot.active_revision = self.exchange.revisions().active.min(snapshot.revision);
        snapshot.engine_power = self.engine_power_status();
        for graph in &mut snapshot.graphs {
            for node in &mut graph.nodes {
                if node.descriptor.capabilities.recorder {
                    // Registry identity is stable across plan swaps. Missing/removed sessions
                    // are represented by their stopped status, never a native lookup.
                    if let Some(id) =
                        recorder_ids.get(&(graph.id.clone(), node.descriptor.node_id.clone()))
                    {
                        node.recorder = Some(self.recorders.status(id));
                    }
                }
            }
        }
        Ok(snapshot)
    }

    pub(super) fn read_snapshot_node_controls(
        &self,
        graph_id: &str,
    ) -> Result<GraphNodeControls, ProtocolError> {
        let snapshot = self.snapshot_controls()?;
        let graph = snapshot
            .graphs
            .into_iter()
            .find(|graph| graph.id == graph_id)
            .ok_or_else(|| crate::graph_catalog::invalid("graph does not exist"))?;
        Ok(GraphNodeControls {
            graph_id: graph.id,
            revision: snapshot.revision,
            nodes: graph
                .nodes
                .into_iter()
                .map(|node| node.descriptor)
                .collect(),
        })
    }
}

fn graph_snapshot(
    id: &str,
    name: &str,
    graph: &GraphDocument,
    layout: &GraphLayout,
) -> GraphControlSnapshot {
    GraphControlSnapshot {
        id: id.to_owned(),
        name: name.to_owned(),
        nodes: graph
            .nodes
            .iter()
            .filter(|node| node_control::controllable(node))
            .filter_map(|node| {
                let mut descriptor = descriptor(node)?;
                if let Some(label) = layout
                    .node_labels
                    .get(node.id.as_str())
                    .or_else(|| layout.node_labels.get(&format!("{}::host-source", node.id)))
                {
                    descriptor.label = Some(label.clone());
                }
                Some(NodeControlSnapshot {
                    descriptor,
                    recorder: None,
                })
            })
            .collect(),
    }
}

/// Returns `None` for a node kind without a protocol name (composition-only kinds).
fn descriptor(node: &NodeSpec) -> Option<NodeControlDescriptor> {
    let Ok(serde_json::Value::String(kind)) = serde_json::to_value(node.config.kind()) else {
        return None;
    };
    Some(NodeControlDescriptor {
        node_id: node.id.as_str().to_owned(),
        label: node.label.clone(),
        kind,
        control: node.control,
        gain_db: node_control::node_gain(&node.config),
        capabilities: NodeControlCapabilities {
            power: true,
            mute: true,
            recorder: matches!(node.config, NodeConfig::Recorder { .. }),
        },
    })
}
