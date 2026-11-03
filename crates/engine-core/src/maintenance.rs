//! Editor close observation and periodic engine checkpoint policy.
use crate::processor_ports::ProcessorEditorCloseObservation;
use crate::{EngineGraphController, NodeConfig};
use mixrelay_audio_graph::NodeId;
use mixrelay_audio_graph::composition::runtime_node_id;
use mixrelay_control_protocol::ProtocolError;
use std::collections::BTreeMap;
#[derive(Default)]
pub struct CloseLedger {
    saved: BTreeMap<NodeId, ProcessorEditorCloseObservation>,
    last_attempt: Option<NodeId>,
}

impl CloseLedger {
    fn next(
        &mut self,
        current: &BTreeMap<NodeId, ProcessorEditorCloseObservation>,
    ) -> Option<(NodeId, ProcessorEditorCloseObservation)> {
        self.saved.retain(|id, _| current.contains_key(id));
        let pending = |(id, observation): &(&NodeId, &ProcessorEditorCloseObservation)| {
            observation.close_generation != 0 && self.saved.get(*id) != Some(*observation)
        };
        // One export per poll bounds control-thread latency; a broken vendor must not
        // starve another closed editor forever. Failed generations remain retryable.
        let candidate = current
            .iter()
            .filter(pending)
            .find(|(id, _)| self.last_attempt.as_ref().is_none_or(|last| *id > last))
            .or_else(|| current.iter().find(pending))
            .map(|(id, observation)| (id.clone(), *observation));
        self.last_attempt = candidate.as_ref().map(|(id, _)| id.clone());
        candidate
    }
}

impl EngineGraphController {
    pub fn maintain_control(&self, ledger: &mut CloseLedger) {
        self.reclaim_recorders();
        self.retry_catalog_restore();
        self.retry_pause_save();
        if !self.catalog_restore.is_pending() {
            self.checkpoint_closed_editor(ledger);
        }
    }
    /// # Errors
    /// Reports recorder finalization or the final processor checkpoint failure.
    pub fn finalize_control(&self) -> Result<(), ProtocolError> {
        let recording = self.recorders.finalize_all();
        let checkpoint = self.checkpoint_processor_state(None);
        recording.and(checkpoint)
    }
    fn checkpoint_closed_editor(&self, ledger: &mut CloseLedger) {
        let _mutation = self.mutation.lock();
        let Some(catalog) = self.catalog_candidate(&self.graph.read()) else {
            return;
        };
        let targets = catalog
            .documents()
            .flat_map(|(graph_id, document)| {
                document
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.config, NodeConfig::Provider(_)))
                    .map(move |node| (runtime_node_id(graph_id, &node.id), (graph_id, &node.id)))
            })
            .collect::<BTreeMap<_, _>>();
        let current = targets
            .keys()
            .filter_map(|runtime_id| {
                self.processor_controls
                    .editor_close_observation(runtime_id)
                    .ok()
                    .map(|observation| (runtime_id.clone(), observation))
            })
            .collect();
        let Some((id, observation)) = ledger.next(&current) else {
            return;
        };
        let Some((graph_id, authored_id)) = targets.get(&id) else {
            return;
        };
        let result = self.checkpoint_graph_processor_state_with(
            graph_id,
            authored_id.as_str(),
            |node, timeout| {
                self.processor_controls
                    .snapshot_state_for_editor_close(node, observation, timeout)
            },
        );
        if result.is_ok() {
            ledger.saved.insert(id, observation);
        }
        // Transaction failure is already exposed through the catalog durability warning.
        // Do not consume the observation or spam logs on each retry.
    }
}
