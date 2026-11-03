//! Explicit, revision-fenced retry for one active failed hosted processor.

use std::time::Duration;

use crate::{EngineGraphController, graph_composition::runtime_node_id};
use mixrelay_audio_graph::{NodeConfig, NodeId};
use mixrelay_control_protocol::{
    ErrorCode, FailedProcessor, GraphApplyResponse, ProcessorRetryRequest, ProtocolError,
};

const RETRY_COOLDOWN: Duration = Duration::from_secs(2);

impl EngineGraphController {
    /// Reports failures only for enabled processors in the currently active execution view.
    pub(crate) fn failed_processor_status(&self) -> Vec<FailedProcessor> {
        if !self.power.enabled() {
            return Vec::new();
        }
        let revision = self.exchange.plan_revisions().active;
        let views = self.execution_views.read();
        let Some(view) = views.get(&revision) else {
            return Vec::new();
        };
        view.origins
            .iter()
            .filter_map(|(runtime_id, origin)| {
                let enabled = view
                    .controls
                    .snapshot(runtime_id)
                    .is_some_and(|(control, _)| control.enabled);
                if !enabled {
                    return None;
                }
                let instance_id = self.processor_controls.failed_worker_instance(runtime_id)?;
                Some(FailedProcessor {
                    graph_id: origin.graph_id.clone(),
                    node_id: origin.node_id.as_str().to_owned(),
                    instance_id: instance_id.to_string(),
                })
            })
            .collect()
    }

    /// Rebuilds the current composition from durable provider state, excluding only the failed
    /// instance from live state capture. Caller-visible edits are never supplied or replaced.
    pub(crate) fn retry_failed_processor_locked(
        &self,
        request: &ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        let revisions = self.exchange.revisions();
        require_current_revisions(
            request,
            revisions.active,
            revisions.submitted,
            self.read_catalog()?.catalog_revision,
        )?;
        let instance_id = parse_instance_id(&request.expected_instance_id)?;
        if !self.power.enabled() {
            return Err(node_error(
                ErrorCode::NodeDisabled,
                &request.graph_id,
                &request.node_id,
            ));
        }

        let document = self.read_catalog_document(&request.graph_id)?;
        validate_retry_target(&document, &request.graph_id, &request.node_id)?;

        let node_id = NodeId::from(request.node_id.as_str());
        let runtime_id = runtime_node_id(&request.graph_id, &node_id);
        let active_revision = self.exchange.plan_revisions().active;
        let active = self
            .execution_views
            .read()
            .get(&active_revision)
            .is_some_and(|view| {
                view.origins.get(&runtime_id).is_some_and(|origin| {
                    origin.graph_id == request.graph_id && origin.node_id == node_id
                }) && view
                    .controls
                    .snapshot(&runtime_id)
                    .is_some_and(|(control, _)| control.enabled)
            });
        if !active {
            return Err(processor_unavailable(
                &request.graph_id,
                &request.node_id,
                "retry target is not in the active execution",
            ));
        }
        self.processor_controls
            .begin_failed_worker_retry(&runtime_id, instance_id, RETRY_COOLDOWN)
            .map_err(|reason| {
                processor_unavailable(&request.graph_id, &request.node_id, &reason.to_string())
            })?;

        let revision = revisions.submitted.checked_add(1).ok_or_else(|| {
            processor_unavailable(&request.graph_id, &request.node_id, "revision overflow")
        })?;
        let reuse = self.processor_controls.reuse_transaction();
        let selected_document = self.graph.read().clone();
        let mut candidate = self.catalog_candidate(&selected_document).ok_or_else(|| {
            processor_unavailable(
                &request.graph_id,
                &request.node_id,
                "graph catalog is unavailable",
            )
        })?;
        let mut selected_document = candidate.active_document().clone();
        selected_document.revision = revision;
        self.validate_recorder_channel_change(&selected_document)?;
        self.preserve_live_processor_state_excluding(&mut selected_document, &runtime_id)?;
        candidate.update_document(selected_document.clone());
        self.reconcile_catalog_devices(&mut candidate)?;
        self.preserve_catalog_provider_state_excluding(&mut candidate, Some(&runtime_id))?;

        let response = self.publish_graph_locked(selected_document, Some(candidate))?;
        reuse.commit();
        Ok(response)
    }
}

fn require_current_revisions(
    request: &ProcessorRetryRequest,
    active_revision: u64,
    submitted_revision: u64,
    catalog_revision: u64,
) -> Result<(), ProtocolError> {
    if request.expected_revision != submitted_revision || active_revision != submitted_revision {
        return Err(stale_revision(active_revision, submitted_revision));
    }
    if request.expected_catalog_revision != catalog_revision {
        return Err(stale_catalog(catalog_revision));
    }
    Ok(())
}

fn validate_retry_target(
    document: &mixrelay_audio_graph::GraphDocument,
    graph_id: &str,
    node_id: &str,
) -> Result<(), ProtocolError> {
    let node = document
        .nodes
        .iter()
        .find(|node| node.id.as_str() == node_id)
        .ok_or_else(|| node_error(ErrorCode::NodeNotFound, graph_id, node_id))?;
    if !node.control.enabled {
        return Err(node_error(ErrorCode::NodeDisabled, graph_id, node_id));
    }
    if !matches!(&node.config, NodeConfig::Provider(_)) {
        return Err(processor_unavailable(
            graph_id,
            node_id,
            "retry target is not a provider",
        ));
    }
    Ok(())
}

fn parse_instance_id(value: &str) -> Result<u64, ProtocolError> {
    let parsed = value
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .ok_or_else(|| ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({"reason":"invalid processor instance identity"}),
        })?;
    Ok(parsed)
}

fn stale_revision(active: u64, submitted: u64) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::GraphRevisionStale,
        parameters: serde_json::json!({"activeRevision":active,"submittedRevision":submitted}),
    }
}

fn stale_catalog(current: u64) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::GraphRevisionStale,
        parameters: serde_json::json!({"catalogRevision":current}),
    }
}

fn node_error(code: ErrorCode, graph_id: &str, node_id: &str) -> ProtocolError {
    ProtocolError {
        code,
        parameters: serde_json::json!({"graphId":graph_id,"nodeId":node_id}),
    }
}

fn processor_unavailable(graph_id: &str, node_id: &str, reason: &str) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::ProcessorUnavailable,
        parameters: serde_json::json!({"graphId":graph_id,"nodeId":node_id,"reason":reason}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_tokens_require_canonical_decimal_u64_text() {
        assert_eq!(parse_instance_id("0").unwrap(), 0);
        assert_eq!(parse_instance_id("18446744073709551615").unwrap(), u64::MAX);
        for invalid in ["", "00", "+1", "-1", "18446744073709551616", "1.0"] {
            assert_eq!(
                parse_instance_id(invalid).unwrap_err().code,
                ErrorCode::GraphInvalid
            );
        }
    }

    #[test]
    fn retry_preconditions_reject_stale_audio_or_catalog_revisions() {
        let request = ProcessorRetryRequest {
            graph_id: "graph-a".into(),
            node_id: "effect".into(),
            expected_revision: 7,
            expected_catalog_revision: 3,
            expected_instance_id: "11".into(),
        };
        assert!(require_current_revisions(&request, 7, 7, 3).is_ok());
        assert_eq!(
            require_current_revisions(&request, 6, 7, 3)
                .unwrap_err()
                .code,
            ErrorCode::GraphRevisionStale
        );
        assert_eq!(
            require_current_revisions(&request, 7, 8, 3)
                .unwrap_err()
                .code,
            ErrorCode::GraphRevisionStale
        );
        assert_eq!(
            require_current_revisions(&request, 7, 7, 4)
                .unwrap_err()
                .code,
            ErrorCode::GraphRevisionStale
        );
    }

    #[test]
    fn retry_target_must_still_be_an_enabled_provider() {
        let mut document = mixrelay_audio_graph::GraphDocument::new(1, vec![], vec![]);
        assert_eq!(
            validate_retry_target(&document, "graph-a", "effect")
                .unwrap_err()
                .code,
            ErrorCode::NodeNotFound
        );
        let mut node = mixrelay_audio_graph::NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("effect"),
            label: None,
            config: NodeConfig::Provider(mixrelay_audio_graph::ProviderNodeConfig {
                type_id: "fixture:provider".into(),
                parameters: serde_json::json!({"preset":"committed"}),
                ports: None,
            }),
        };
        node.control.enabled = false;
        document.nodes.push(node.clone());
        assert_eq!(
            validate_retry_target(&document, "graph-a", "effect")
                .unwrap_err()
                .code,
            ErrorCode::NodeDisabled
        );
        node.control.enabled = true;
        node.config = NodeConfig::Gain(mixrelay_audio_graph::GainConfig {
            channels: 2,
            gain_db: 0.0,
        });
        document.nodes[0] = node;
        assert_eq!(
            validate_retry_target(&document, "graph-a", "effect")
                .unwrap_err()
                .code,
            ErrorCode::ProcessorUnavailable
        );
    }
}
