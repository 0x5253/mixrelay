//! Graph edits carry explicit knob deltas, never an implicit replacement of a live preset.
//! The UI's whole provider object may predate native-editor edits or a disk checkpoint.

use std::{collections::BTreeSet, time::Duration};

use crate::processor_ports::{PROCESSOR_STATE_TIMEOUT, ProcessorStateError};
use mixrelay_audio_graph::{GraphDocument, NodeId, ProviderNodeConfig};
use mixrelay_control_protocol::GraphProcessorParameterChange;

use super::{
    Catalog, EngineGraphController, ErrorCode, NodeConfig, ProtocolError, invalid, unavailable,
};
use crate::graph_composition::runtime_node_id;

/// An explicit edit captures all live state; a retry preserves the failed instance's checkpoint.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LiveStateIntent<'a> {
    Edit(&'a [GraphProcessorParameterChange]),
    Retry(&'a NodeId),
}

impl EngineGraphController {
    /// Caller owns mutation. Captures the selected graph before candidate preparation.
    pub(crate) fn preserve_live_processor_state(
        &self,
        proposed: &mut GraphDocument,
        changes: &[GraphProcessorParameterChange],
    ) -> Result<(), ProtocolError> {
        self.preserve_live_processor_state_with(
            proposed,
            changes,
            |id, timeout| self.processor_controls.snapshot_state(id, timeout),
            |kind, id| self.processor_catalog.parameter_is_writable(kind, id),
        )
    }

    pub(crate) fn preserve_live_processor_state_with(
        &self,
        proposed: &mut GraphDocument,
        changes: &[GraphProcessorParameterChange],
        capture: impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
        writable: impl Fn(&str, u32) -> bool,
    ) -> Result<(), ProtocolError> {
        self.preserve_live_processor_state_for(
            proposed,
            LiveStateIntent::Edit(changes),
            capture,
            writable,
        )
    }

    pub(crate) fn preserve_live_processor_state_excluding(
        &self,
        proposed: &mut GraphDocument,
        excluded: &NodeId,
    ) -> Result<(), ProtocolError> {
        self.preserve_live_processor_state_for(
            proposed,
            LiveStateIntent::Retry(excluded),
            |id, timeout| self.processor_controls.snapshot_state(id, timeout),
            |kind, id| self.processor_catalog.parameter_is_writable(kind, id),
        )
    }

    pub(crate) fn preserve_live_processor_state_for(
        &self,
        proposed: &mut GraphDocument,
        intent: LiveStateIntent<'_>,
        mut capture: impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
        writable: impl Fn(&str, u32) -> bool,
    ) -> Result<(), ProtocolError> {
        let (changes, excluded): (&[GraphProcessorParameterChange], Option<&NodeId>) = match intent
        {
            LiveStateIntent::Edit(changes) => (changes, None),
            LiveStateIntent::Retry(node_id) => (&[], Some(node_id)),
        };
        let revisions = self.exchange.revisions();
        validate_capture_revision(proposed.revision, revisions.active, revisions.submitted)?;
        let current = self.graph.read().clone();
        let graph_id = self.view_graph_id();
        let restoring = self.catalog_restore.is_pending();
        validate_changes(&current, proposed, changes, writable)?;
        let mut candidate = proposed.clone();
        for node in &mut candidate.nodes {
            let NodeConfig::Provider(provider) = &mut node.config else {
                continue;
            };
            let Some(previous_node) = current.nodes.iter().find(|before| before.id == node.id)
            else {
                continue;
            };
            let NodeConfig::Provider(previous) = &previous_node.config else {
                continue;
            };
            if previous.type_id != provider.type_id {
                continue;
            }
            let runtime_id = runtime_node_id(&graph_id, &node.id);
            let unavailable_worker =
                previous.ports.is_some() && !self.processor_controls.has_worker(&runtime_id);
            if unavailable_worker
                && self.power.enabled()
                && changes
                    .iter()
                    .any(|change| change.node_id == node.id.as_str())
            {
                return Err(ProtocolError {
                    code: ErrorCode::ProcessorUnavailable,
                    parameters: serde_json::json!({"nodeId": node.id.as_str(), "reason": "processor worker is unavailable"}),
                });
            }
            if excluded == Some(&runtime_id) {
                provider.parameters.clone_from(&previous.parameters);
            } else if self.power.enabled()
                && !unavailable_worker
                && (!restoring || self.provider_is_running(&runtime_id))
            {
                provider.parameters = self
                    .capture_node_state(
                        (&runtime_id, node.control.enabled),
                        PROCESSOR_STATE_TIMEOUT,
                        &mut capture,
                    )?
                    .unwrap_or_else(|| previous.parameters.clone());
            } else {
                provider.parameters.clone_from(&previous.parameters);
            }
            for change in changes
                .iter()
                .filter(|change| change.node_id == node.id.as_str())
            {
                crate::persist_processor_parameter(provider, change.parameter_id, change.value);
            }
        }
        // No partial rewrite is visible if any provider failed to export its state.
        *proposed = candidate;
        Ok(())
    }

    /// Preserves live presets in other running graphs before rebuilding the composition.
    /// The selected graph already carries its explicitly validated edit deltas.
    pub(crate) fn preserve_catalog_provider_state(
        &self,
        candidate: &mut Catalog,
    ) -> Result<(), ProtocolError> {
        self.preserve_catalog_provider_state_excluding(candidate, None)
    }

    pub(crate) fn preserve_catalog_provider_state_excluding(
        &self,
        candidate: &mut Catalog,
        excluded: Option<&NodeId>,
    ) -> Result<(), ProtocolError> {
        self.preserve_catalog_provider_state_with_exclusion(
            candidate,
            |id, timeout| self.processor_controls.snapshot_state(id, timeout),
            excluded,
        )
    }

    pub(crate) fn preserve_catalog_provider_state_with_exclusion(
        &self,
        candidate: &mut Catalog,
        mut capture: impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
        excluded: Option<&NodeId>,
    ) -> Result<(), ProtocolError> {
        let current = self
            .catalog_candidate(&self.graph.read())
            .ok_or_else(|| unavailable("graph catalog is unavailable"))?;
        let selected = current.selected_id();
        let mut staged = candidate.clone();
        for (graph_id, document) in staged.documents_mut() {
            if graph_id == selected {
                continue;
            }
            let Some((_, before)) = current.documents().find(|(id, _)| *id == graph_id) else {
                continue;
            };
            self.capture_background_graph_excluding(
                (graph_id, document),
                before,
                &mut capture,
                excluded,
            )?;
        }
        *candidate = staged;
        Ok(())
    }

    pub(super) fn capture_background_graph(
        &self,
        graph: (&str, &mut GraphDocument),
        before: &GraphDocument,
        capture: &mut impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<(), ProtocolError> {
        self.capture_background_graph_excluding(graph, before, capture, None)
    }

    fn capture_background_graph_excluding(
        &self,
        graph: (&str, &mut GraphDocument),
        before: &GraphDocument,
        capture: &mut impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
        excluded: Option<&NodeId>,
    ) -> Result<(), ProtocolError> {
        let (graph_id, document) = graph;
        for node in &mut document.nodes {
            let NodeConfig::Provider(provider) = &mut node.config else {
                continue;
            };
            let runtime_id = runtime_node_id(graph_id, &node.id);
            if excluded == Some(&runtime_id) {
                continue;
            }
            if provider.ports.is_some() && !self.processor_controls.has_worker(&runtime_id) {
                continue;
            }
            // Saved documents are not live workers until their composition is acknowledged.
            let running = self.provider_is_running(&runtime_id);
            let same_provider = provider_at(before, node.id.as_str())
                .is_some_and(|old| old.type_id == provider.type_id);
            if !running || !same_provider {
                continue;
            }
            if let Some(parameters) = self.capture_node_state(
                (&runtime_id, node.control.enabled),
                PROCESSOR_STATE_TIMEOUT,
                capture,
            )? {
                provider.parameters = parameters;
            }
        }
        Ok(())
    }

    fn provider_is_running(&self, runtime_id: &NodeId) -> bool {
        self.execution_views
            .read()
            .get(&self.exchange.plan_revisions().active)
            .is_some_and(|view| view.origins.contains_key(runtime_id))
    }

    fn capture_node_state(
        &self,
        target: (&NodeId, bool),
        timeout: Duration,
        capture: &mut impl FnMut(&NodeId, Duration) -> Result<serde_json::Value, ProcessorStateError>,
    ) -> Result<Option<serde_json::Value>, ProtocolError> {
        match capture(target.0, timeout) {
            Ok(parameters) => Ok(Some(parameters)),
            Err(error) if !target.1 => {
                // Turning off a failed worker must remain possible. Retain the last
                // durable preset and surface the failed checkpoint independently.
                let reason = format!("disabled processor retained saved state: {error}");
                tracing::warn!(node_id = %target.0, %reason);
                *self.node_control_warning.lock() = Some(reason);
                Ok(None)
            }
            Err(error) => Err(unavailable(&format!(
                "processor state snapshot failed: {error}"
            ))),
        }
    }
}

fn validate_capture_revision(
    proposed: u64,
    active: u64,
    submitted: u64,
) -> Result<(), ProtocolError> {
    if proposed <= submitted {
        return Err(ProtocolError {
            code: ErrorCode::GraphRevisionStale,
            parameters: serde_json::json!({
                "activeRevision": active,
                "submittedRevision": submitted,
                "proposedRevision": proposed,
            }),
        });
    }
    if active != submitted {
        return Err(ProtocolError {
            code: ErrorCode::GraphExchangeBusy,
            parameters: serde_json::json!({"proposedRevision": proposed}),
        });
    }
    Ok(())
}

fn provider_at<'a>(document: &'a GraphDocument, id: &str) -> Option<&'a ProviderNodeConfig> {
    document.nodes.iter().find_map(|node| {
        if node.id.as_str() != id {
            return None;
        }
        match &node.config {
            NodeConfig::Provider(provider) => Some(provider),
            _ => None,
        }
    })
}

fn validate_changes(
    current: &GraphDocument,
    proposed: &GraphDocument,
    changes: &[GraphProcessorParameterChange],
    writable: impl Fn(&str, u32) -> bool,
) -> Result<(), ProtocolError> {
    if changes.len() > 4096 {
        return Err(invalid("too many processor parameter changes"));
    }
    let mut unique = BTreeSet::new();
    for change in changes {
        if current
            .nodes
            .iter()
            .any(|node| node.id.as_str() == change.node_id && !node.control.enabled)
        {
            return Err(crate::node_control::node_error(
                ErrorCode::NodeDisabled,
                "",
                &change.node_id,
            ));
        }
        if !change.value.is_finite()
            || !(0.0..=1.0).contains(&change.value)
            || !unique.insert((&change.node_id, change.parameter_id))
        {
            return Err(invalid("invalid or duplicate processor parameter change"));
        }
        let before = provider_at(current, &change.node_id);
        let after = provider_at(proposed, &change.node_id);
        if !matches!((before, after), (Some(a), Some(b))
            if a.type_id.as_str() == change.type_id && b.type_id == a.type_id)
            || !writable(&change.type_id, change.parameter_id)
        {
            return Err(invalid(
                "processor parameter target is unavailable or read-only",
            ));
        }
    }
    Ok(())
}
