//! Maps one authored node edit onto a coherent batch of existing runtime controls.
//! Shared native owners follow the OR of local references; signal gain/mute remain local.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use mixrelay_audio_engine::{RuntimeControlBank, RuntimeControlError, RuntimeControlUpdate};
use mixrelay_audio_graph::{NodeConfig, NodeControl, NodeId};
use mixrelay_control_protocol::{
    ApplicationCaptureManifest, GraphApplyResponse, GraphApplyState, NodeControlRequest,
    PROTOCOL_VERSION, ProtocolError,
};

use crate::{
    EngineGraphController,
    execution_control::ExecutionView,
    graph_catalog::{Catalog, invalid},
};

pub(super) struct RuntimePatch {
    bank: Arc<RuntimeControlBank>,
    updates: Vec<RuntimeControlUpdate>,
    pub capture_manifest: Option<ApplicationCaptureManifest>,
}

impl RuntimePatch {
    fn enabled(&self, id: &NodeId) -> bool {
        self.updates
            .iter()
            .find(|update| &update.node_id == id)
            .map_or_else(
                || {
                    self.bank
                        .snapshot(id)
                        .is_some_and(|(control, _)| control.enabled)
                },
                |update| update.control.enabled,
            )
    }

    /// Updates shared owners only when all required native resources already exist.
    fn include_native_owners(&mut self, view: &ExecutionView) -> bool {
        for (owner, references) in &view.native_controls {
            if !references
                .iter()
                .any(|id| self.updates.iter().any(|update| &update.node_id == id))
            {
                continue;
            }
            let enabled = references.iter().any(|id| self.enabled(id));
            if enabled && !view.prepared_native.contains(owner) {
                return false;
            }
            self.updates.push(RuntimeControlUpdate {
                node_id: owner.clone(),
                control: NodeControl {
                    enabled,
                    muted: false,
                },
                gain_db: None,
            });
        }
        true
    }

    fn capture_manifest(
        &self,
        view: &ExecutionView,
        revision: u64,
    ) -> Result<ApplicationCaptureManifest, ProtocolError> {
        let mut manifest = view.captures.clone();
        manifest.revision = revision;
        crate::execution_control::apply_playback_intents(
            &mut manifest,
            &view.playback_intents,
            |id| {
                self.updates
                    .iter()
                    .find(|update| &update.node_id == id)
                    .map(|update| update.control)
                    .or_else(|| self.bank.snapshot(id).map(|(control, _)| control))
            },
        )?;
        manifest
            .sources
            .retain(|source| self.enabled(&NodeId::from(source.node_id.as_str())));
        Ok(manifest)
    }
}

impl EngineGraphController {
    pub(super) fn runtime_control_patch(
        &self,
        candidate: &Catalog,
        graph_id: &str,
        node_id: &str,
        request: &NodeControlRequest,
    ) -> Result<Option<RuntimePatch>, ProtocolError> {
        let views = self.execution_views.read();
        let Some(view) = views.get(&self.exchange.plan_revisions().submitted) else {
            return Ok(None);
        };
        let node = candidate
            .documents()
            .find(|(id, _)| *id == graph_id)
            .and_then(|(_, document)| {
                document
                    .nodes
                    .iter()
                    .find(|node| node.id.as_str() == node_id)
            })
            .ok_or_else(|| invalid("node does not exist"))?;
        let updates: Vec<_> = view
            .origins
            .iter()
            .filter(|(_, origin)| origin.graph_id == graph_id && origin.node_id.as_str() == node_id)
            .map(|(id, origin)| RuntimeControlUpdate {
                node_id: id.clone(),
                control: node.control,
                gain_db: if matches!(node.config, NodeConfig::VirtualEndpoint(_))
                    && origin
                        .meter_port
                        .as_ref()
                        .is_some_and(|port| port.as_str() == mixrelay_audio_graph::TO_OS_PORT_ID)
                {
                    None
                } else {
                    request.gain_db
                },
            })
            .collect();
        if updates.is_empty() {
            return Ok(None);
        }
        if matches!(node.config, NodeConfig::ApplicationCapture(_))
            && node.control.enabled
            && !view.native_controls.values().any(|references| {
                references
                    .iter()
                    .any(|id| updates.iter().any(|update| &update.node_id == id))
            })
        {
            return Ok(None);
        }
        let mut patch = RuntimePatch {
            bank: Arc::clone(&view.controls),
            updates,
            capture_manifest: None,
        };
        if !patch.include_native_owners(view) {
            return Ok(None);
        }
        if (request.enabled.is_some()
            || (request.muted.is_some()
                && view.playback_intents.iter().any(|intent| {
                    patch
                        .updates
                        .iter()
                        .any(|update| update.node_id == intent.mask_node_id)
                })))
            && matches!(node.config, NodeConfig::ApplicationCapture(_))
        {
            patch.capture_manifest =
                Some(patch.capture_manifest(view, candidate.active_document().revision)?);
        }
        match patch
            .bank
            .validate(candidate.active_document().revision, &patch.updates)
        {
            Ok(()) => Ok(Some(patch)),
            Err(RuntimeControlError::NeedsPreparation(_)) => Ok(None),
            Err(error) => Err(invalid(&error.to_string())),
        }
    }

    pub(super) fn publish_runtime_controls(
        &self,
        candidate: Catalog,
        patch: &RuntimePatch,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let document = candidate.active_document().clone();
        let revision = document.revision;
        let staged = self.stage_catalog(Some(&candidate))?;
        self.exchange
            .publish_controls(&patch.bank, revision, &patch.updates)
            .map_err(|error| invalid(&error.to_string()))?;
        self.stop_inactive_recorders(&document, Some(&candidate));
        self.finish_catalog_write(staged);
        *self.graph.write() = document;
        self.commit_catalog_memory(Some(candidate));
        Ok(GraphApplyResponse {
            protocol_version: PROTOCOL_VERSION,
            revision,
            state: if self.exchange.revisions().active >= revision {
                GraphApplyState::Active
            } else {
                GraphApplyState::Pending
            },
            persistence_error: self.graph_persistence_warning(),
        })
    }
}
