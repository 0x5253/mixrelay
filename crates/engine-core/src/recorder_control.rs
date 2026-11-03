use mixrelay_audio_graph::NodeConfig;
use mixrelay_control_protocol::{ErrorCode, ProtocolError};

use super::{EngineGraphController, graph_composition, recorder_sessions::recorder_error};

impl EngineGraphController {
    pub(super) fn stop_inactive_recorders(
        &self,
        proposed: &mixrelay_audio_graph::GraphDocument,
        catalog: Option<&crate::graph_catalog::Catalog>,
    ) {
        let current = self.graph.read().clone();
        let before = self.catalog_candidate(&current);
        let documents: Vec<_> = before.as_ref().map_or_else(
            || vec![(self.view_graph_id(), &current)],
            |value| {
                value
                    .documents()
                    .map(|(id, graph)| (id.to_owned(), graph))
                    .collect()
            },
        );
        for (graph_id, document) in documents {
            let next = catalog.map_or(Some(proposed), |value| {
                value
                    .documents()
                    .find(|(id, _)| *id == graph_id)
                    .map(|(_, graph)| graph)
            });
            for node in &document.nodes {
                let NodeConfig::Recorder { channels } = node.config else {
                    continue;
                };
                if !node.control.enabled || next.is_some_and(|graph| graph.nodes.iter().any(|next| {
                    next.id == node.id && next.control.enabled
                        && matches!(next.config, NodeConfig::Recorder { channels: width } if channels == width)
                })) { continue; }
                let id = mixrelay_render::recording_session_key(
                    &graph_composition::runtime_node_id(&graph_id, &node.id),
                    channels,
                );
                if !matches!(
                    self.recorders.status(&id).state,
                    mixrelay_audio_io::recording::RecorderState::Recording
                        | mixrelay_audio_io::recording::RecorderState::Paused
                ) {
                    continue;
                }
                self.recorders.stop_inactive(&id, &graph_id, &node.id);
            }
        }
    }

    pub(super) fn reclaim_recorders(&self) {
        self.exchange.reclaim_retired();
        self.recorders.reclaim();
    }
    pub(super) fn recorder_session_id(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<String, ProtocolError> {
        let document = self.catalog_document_locked(graph_id)?;
        let (id, channels) = document
            .nodes
            .iter()
            .find(|node| node.id.as_str() == node_id)
            .and_then(|node| match node.config {
                NodeConfig::Recorder { channels } => Some((&node.id, channels)),
                _ => None,
            })
            .ok_or_else(|| {
                crate::node_control::node_error(ErrorCode::NodeNotFound, graph_id, node_id)
            })?;
        Ok(mixrelay_render::recording_session_key(
            &graph_composition::runtime_node_id(graph_id, id),
            channels,
        ))
    }

    pub(super) fn validate_recorder_channel_change(
        &self,
        proposed: &mixrelay_audio_graph::GraphDocument,
    ) -> Result<(), ProtocolError> {
        let graph_id = self.view_graph_id();
        for previous in &self.graph.read().nodes {
            let NodeConfig::Recorder { channels } = previous.config else {
                continue;
            };
            let changed = proposed.nodes.iter().any(|node| {
                node.id == previous.id
                    && matches!(node.config, NodeConfig::Recorder { channels: next } if next != channels)
            });
            if !changed {
                continue;
            }
            let id = mixrelay_render::recording_session_key(
                &graph_composition::runtime_node_id(&graph_id, &previous.id),
                channels,
            );
            if matches!(
                self.recorders.status(&id).state,
                mixrelay_audio_io::recording::RecorderState::Recording
                    | mixrelay_audio_io::recording::RecorderState::Paused
            ) {
                return Err(recorder_error("stop recording before changing channels"));
            }
        }
        Ok(())
    }
}
