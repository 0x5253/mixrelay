//! Explicit control-thread checkpoints for edits made inside a vendor's native editor.
//! A checkpoint has no audio publication: stage and replacement must both succeed before
//! publishing the captured preset to the in-memory document. A failed close can then retry
//! against the same live instance instead of pretending that an unsaved preset was stored.

use std::time::Duration;

use crate::graph_composition::runtime_node_id;
use crate::processor_ports::{PROCESSOR_STATE_TIMEOUT, ProcessorStateError};
use mixrelay_audio_graph::NodeId;

use super::{EngineGraphController, NodeConfig, ProtocolError, invalid, unavailable};

impl EngineGraphController {
    /// Saves a selected graph's authored provider, or every running graph's provider for `None`.
    /// Call from an explicit control-plane lifecycle before closing a native editor or
    /// stopping the service. This must never run on the real-time audio thread.
    ///
    /// # Errors
    /// Rejects pending audio publication when native providers need a checkpoint,
    /// unavailable/unsupported native state, and any
    /// storage failure. Existing graph/catalog documents remain unchanged on failure.
    pub fn checkpoint_processor_state(&self, node_id: Option<&str>) -> Result<(), ProtocolError> {
        let _mutation = self.mutation.lock();
        self.checkpoint_processor_state_locked(node_id)
    }

    /// The caller must own `mutation`, including any following native close publication.
    pub(crate) fn checkpoint_processor_state_locked(
        &self,
        node_id: Option<&str>,
    ) -> Result<(), ProtocolError> {
        if !self.power.enabled() {
            return Ok(());
        }
        if node_id.is_none() && self.catalog_restore.is_pending() {
            return Ok(());
        }
        self.checkpoint_processor_state_with(node_id, |id, remaining| {
            self.processor_controls.snapshot_state(id, remaining)
        })
    }

    // Capture injection keeps lifecycle/storage tests deterministic without loading a
    // third-party binary. The production entry point always uses the live worker registry.
    pub(crate) fn checkpoint_processor_state_with(
        &self,
        node_id: Option<&str>,
        capture: impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<(), ProtocolError> {
        let graph_id = self.view_graph_id();
        self.checkpoint_provider_target(node_id.map(|id| (graph_id.as_str(), id)), capture)
    }

    /// Checkpoints one exact authored owner of an observed runtime editor close.
    /// Caller holds the mutation lock through observation, export and durable commit.
    pub(crate) fn checkpoint_graph_processor_state_with(
        &self,
        graph_id: &str,
        node_id: &str,
        capture: impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<(), ProtocolError> {
        self.checkpoint_provider_target(Some((graph_id, node_id)), capture)
    }

    fn checkpoint_provider_target(
        &self,
        target: Option<(&str, &str)>,
        mut capture: impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<(), ProtocolError> {
        let reuse = self.processor_controls.reuse_transaction();
        let result = self.checkpoint_processor_state_transaction(target, &mut capture);
        if let Err(error) = &result {
            self.catalog
                .record_warning(Some(format!("native state checkpoint failed: {error:?}")));
        }
        result?;
        reuse.commit();
        Ok(())
    }

    fn checkpoint_processor_state_transaction(
        &self,
        target: Option<(&str, &str)>,
        capture: &mut impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<(), ProtocolError> {
        let document = self.graph.read().clone();
        let Some(mut candidate) = self.catalog_candidate(&document) else {
            return if target.is_none()
                && !document
                    .nodes
                    .iter()
                    .any(|node| matches!(node.config, NodeConfig::Provider(_)))
            {
                Ok(())
            } else {
                Err(unavailable("graph catalog is unavailable"))
            };
        };
        // Shutdown can race the first restored audio revision. With no native
        // provider in any graph there is nothing to export or rewrite; keep the
        // already durable catalog untouched. Real provider snapshots still need
        // the exact active publication before inspecting any worker instance.
        if target.is_none()
            && !candidate.documents().any(|(_, graph)| {
                graph
                    .nodes
                    .iter()
                    .any(|node| matches!(node.config, NodeConfig::Provider(_)))
            })
        {
            return Ok(());
        }
        let revisions = self.exchange.revisions();
        if revisions.active != revisions.submitted {
            return Err(unavailable(
                "native state checkpoint requires an active graph",
            ));
        }
        let mut found = false;
        for (graph_id, graph) in candidate.documents_mut() {
            found |= self.capture_document((graph_id, graph), target, capture)?;
        }
        if !found {
            return if target.is_some() {
                Err(invalid("processor node does not exist"))
            } else {
                Ok(())
            };
        }
        if let Some(staged) = self.stage_catalog(Some(&candidate))? {
            staged
                .commit()
                .map_err(|error| unavailable(&error.to_string()))?;
        }
        *self.graph.write() = candidate.active_document().clone();
        self.commit_catalog_memory(Some(candidate));
        self.catalog.record_warning(None);
        Ok(())
    }
    fn capture_document(
        &self,
        graph: (&str, &mut mixrelay_audio_graph::GraphDocument),
        target: Option<(&str, &str)>,
        capture: &mut impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<bool, ProtocolError> {
        let (graph_id, document) = graph;
        let mut found = false;
        for node in &mut document.nodes {
            if target.is_some_and(|(graph, id)| graph != graph_id || id != node.id.as_str()) {
                continue;
            }
            let NodeConfig::Provider(provider) = &mut node.config else {
                continue;
            };
            if !node.control.enabled
                && !self
                    .processor_controls
                    .has_worker(&runtime_node_id(graph_id, &node.id))
            {
                found = true;
                continue;
            }
            if provider.ports.is_some()
                && !self
                    .processor_controls
                    .has_worker(&runtime_node_id(graph_id, &node.id))
            {
                found = true;
                continue;
            }
            // Replace the whole preset: stale parameter overlays must not undo editor edits.
            provider.parameters = capture(
                &runtime_node_id(graph_id, &node.id),
                PROCESSOR_STATE_TIMEOUT,
            )
            .map_err(|error| unavailable(&format!("processor state snapshot failed: {error}")))?;
            found = true;
        }
        Ok(found)
    }
}
