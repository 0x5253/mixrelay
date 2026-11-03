//! Authored controls publish to stable runtime slots; native preparation is explicit.

use mixrelay_audio_graph::{GraphDocument, NodeConfig, NodeSpec};
use mixrelay_control_protocol::{
    ErrorCode, GraphApplyResponse, GraphNodeControls, NodeControlPreview, NodeControlRequest,
    NodeControlResponse, ProtocolError,
};

use crate::{
    EngineGraphController,
    graph_catalog::{Catalog, invalid, unavailable},
};

impl EngineGraphController {
    /// Bounded writer admission; the revision fence is checked after admission.
    pub(super) fn try_node_control(
        &self,
    ) -> Result<parking_lot::MutexGuard<'_, ()>, ProtocolError> {
        self.mutation
            .try_lock_for(std::time::Duration::from_millis(250))
            .ok_or_else(|| ProtocolError {
                code: ErrorCode::GraphExchangeBusy,
                parameters: serde_json::json!({"reason":"graph mutation in progress"}),
            })
    }
    pub(super) fn validate_control_revision(&self, expected: u64) -> Result<(), ProtocolError> {
        let revisions = self.exchange.revisions();
        if expected != revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({"submittedRevision": revisions.submitted}),
            });
        }
        if revisions.active != revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphExchangeBusy,
                parameters: serde_json::json!({"activeRevision": revisions.active}),
            });
        }
        Ok(())
    }
    pub(super) fn read_node_controls(
        &self,
        graph_id: &str,
    ) -> Result<GraphNodeControls, ProtocolError> {
        self.read_snapshot_node_controls(graph_id)
    }

    pub(super) fn preview_node_switch(
        &self,
        graph_id: &str,
        node_id: &str,
        request: &NodeControlRequest,
    ) -> Result<NodeControlPreview, ProtocolError> {
        let _mutation = self.try_node_control()?;
        let candidate = self.node_control_candidate(graph_id, node_id, request)?;
        if self.control_is_unchanged(graph_id, node_id, request)? {
            return Ok(NodeControlPreview {
                capture_manifest: None,
            });
        }
        if let Some(patch) = self.runtime_control_patch(&candidate, graph_id, node_id, request)? {
            return Ok(NodeControlPreview {
                capture_manifest: patch.capture_manifest,
            });
        }
        let composed = self.compose_execution(
            candidate.active_document(),
            Some(&candidate),
            *self.context.read(),
        )?;
        Ok(NodeControlPreview {
            capture_manifest: Some(crate::execution_control::composed_capture_manifest(
                &composed,
            )?),
        })
    }

    pub(super) fn change_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: &NodeControlRequest,
    ) -> Result<NodeControlResponse, ProtocolError> {
        let _mutation = self.try_node_control()?;
        let mut candidate = self.node_control_candidate(graph_id, node_id, request)?;
        if self.control_is_unchanged(graph_id, node_id, request)? {
            return Ok(NodeControlResponse {
                application: GraphApplyResponse {
                    protocol_version: mixrelay_control_protocol::PROTOCOL_VERSION,
                    revision: self.exchange.revisions().submitted,
                    state: mixrelay_control_protocol::GraphApplyState::Active,
                    persistence_error: self.graph_persistence_warning(),
                },
                requires_graph_reload: false,
            });
        }
        if let Some(patch) = self.runtime_control_patch(&candidate, graph_id, node_id, request)? {
            return self
                .publish_runtime_controls(candidate, &patch)
                .map(|application| NodeControlResponse {
                    application,
                    requires_graph_reload: false,
                });
        }
        let reuse = self.processor_controls.reuse_transaction();
        let mut selected = candidate.active_document().clone();
        self.preserve_live_processor_state(&mut selected, &[])?;
        candidate.update_document(selected.clone());
        let application = self.apply_graph_locked(selected, Some(candidate))?;
        reuse.commit();
        Ok(NodeControlResponse {
            application,
            requires_graph_reload: true,
        })
    }

    fn node_control_candidate(
        &self,
        graph_id: &str,
        node_id: &str,
        request: &NodeControlRequest,
    ) -> Result<Catalog, ProtocolError> {
        self.validate_control_revision(request.expected_revision)?;
        let revisions = self.exchange.revisions();
        if request.enabled.is_none() && request.muted.is_none() && request.gain_db.is_none() {
            return Err(invalid("node control update is empty"));
        }
        let mut candidate = self
            .catalog_candidate(&self.graph.read())
            .ok_or_else(|| unavailable("graph catalog is unavailable"))?;
        let (_, graph) = candidate
            .documents_mut()
            .find(|(id, _)| *id == graph_id)
            .ok_or_else(|| invalid("graph does not exist"))?;
        let node = graph
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == node_id)
            .filter(|node| controllable(node))
            .ok_or_else(|| node_error(ErrorCode::NodeNotFound, graph_id, node_id))?;
        if let Some(enabled) = request.enabled {
            node.control.enabled = enabled;
        }
        if let Some(muted) = request.muted {
            node.control.muted = muted;
        }
        if let Some(gain) = request.gain_db {
            set_node_gain(&mut node.config, gain)?;
        }
        let next = revisions
            .submitted
            .checked_add(1)
            .ok_or_else(|| invalid("graph revision exhausted"))?;
        graph.revision = next;
        let mut selected: GraphDocument = candidate.active_document().clone();
        selected.revision = next;
        candidate.update_document(selected);
        Ok(candidate)
    }

    fn control_is_unchanged(
        &self,
        graph_id: &str,
        node_id: &str,
        request: &NodeControlRequest,
    ) -> Result<bool, ProtocolError> {
        let graph = self.catalog_document_locked(graph_id)?;
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id.as_str() == node_id)
            .ok_or_else(|| node_error(ErrorCode::NodeNotFound, graph_id, node_id))?;
        Ok(request
            .enabled
            .is_none_or(|enabled| node.control.enabled == enabled)
            && request
                .muted
                .is_none_or(|muted| node.control.muted == muted)
            && request
                .gain_db
                .is_none_or(|gain| node_gain(&node.config) == Some(gain)))
    }
}

pub(super) fn node_error(code: ErrorCode, graph_id: &str, node_id: &str) -> ProtocolError {
    ProtocolError {
        code,
        parameters: serde_json::json!({"graphId": graph_id, "nodeId": node_id}),
    }
}

pub(super) fn controllable(node: &NodeSpec) -> bool {
    !matches!(
        node.config,
        NodeConfig::VirtualEndpointTransport(_) | NodeConfig::DeviceBus { .. }
    )
}

pub(super) fn node_gain(config: &NodeConfig) -> Option<f32> {
    match config {
        NodeConfig::Gain(value) => Some(value.gain_db),
        NodeConfig::VirtualEndpoint(value) => Some(value.gain_db),
        NodeConfig::AudioDeviceInput(value) => Some(value.gain_db),
        NodeConfig::ApplicationCapture(value) => Some(value.gain_db),
        _ => None,
    }
}

fn set_node_gain(config: &mut NodeConfig, gain: f32) -> Result<(), ProtocolError> {
    if !gain.is_finite() || !(-60.0..=12.0).contains(&gain) {
        return Err(invalid("gain must be between -60 and 12 dB"));
    }
    let target = match config {
        NodeConfig::Gain(value) => &mut value.gain_db,
        NodeConfig::VirtualEndpoint(value) => &mut value.gain_db,
        NodeConfig::AudioDeviceInput(value) => &mut value.gain_db,
        NodeConfig::ApplicationCapture(value) => &mut value.gain_db,
        _ => return Err(invalid("node does not support gain")),
    };
    *target = gain;
    Ok(())
}

#[cfg(test)]
mod tests;
