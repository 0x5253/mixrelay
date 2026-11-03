//! Live processor admission and persistence of accepted parameter changes.

use crate::{
    EngineGraphController, ErrorCode, NodeConfig, NodeId, PROTOCOL_VERSION,
    ProcessorParameterUpdateResponse, ProtocolError, graph_composition, node_control,
};

impl EngineGraphController {
    /// Native publication precedes durable control-plane bookkeeping. Saving an accepted
    /// parameter never prepares a graph, increments its revision, or replaces a live worker.
    pub(super) fn apply_processor_parameter_update(
        &self,
        node_id: &str,
        parameter_id: u32,
        value: f64,
        publish: impl FnOnce(
            &mixrelay_audio_graph::ProviderNodeConfig,
            &NodeId,
        ) -> Result<(), ProtocolError>,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(processor_parameter_error(
                node_id,
                parameter_id,
                "parameter value must be normalized between 0 and 1",
            ));
        }
        let runtime_id =
            graph_composition::runtime_node_id(&self.view_graph_id(), &NodeId::from(node_id));
        if !self.power.enabled()
            || !self
                .graph
                .read()
                .nodes
                .iter()
                .any(|node| node.id.as_str() == node_id && node.control.enabled)
        {
            let code = if self
                .graph
                .read()
                .nodes
                .iter()
                .any(|node| node.id.as_str() == node_id)
            {
                ErrorCode::NodeDisabled
            } else {
                ErrorCode::NodeNotFound
            };
            return Err(node_control::node_error(
                code,
                &self.view_graph_id(),
                node_id,
            ));
        }
        let mut graph = self.graph.write();
        let provider = graph
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == node_id)
            .and_then(|node| match &mut node.config {
                NodeConfig::Provider(provider) => Some(provider),
                _ => None,
            })
            .ok_or_else(|| {
                processor_parameter_error(node_id, parameter_id, "processor node does not exist")
            })?;
        publish(provider, &runtime_id)?;
        persist_processor_parameter(provider, parameter_id, value);
        let document = graph.clone();
        drop(graph);
        let persistence_error = self
            .persist_parameter_document(&document)
            .or_else(|| self.power.warning());
        Ok(ProcessorParameterUpdateResponse {
            protocol_version: PROTOCOL_VERSION,
            node_id: node_id.to_owned(),
            parameter_id,
            value,
            persistence_error,
        })
    }
}
impl EngineGraphController {
    pub(super) fn external_processor_node(&self, node_id: &str) -> Result<NodeId, ProtocolError> {
        if !self.power.enabled() {
            return Err(node_control::node_error(
                ErrorCode::NodeDisabled,
                &self.view_graph_id(),
                node_id,
            ));
        }
        let node_id = NodeId::from(node_id);
        let runtime_id = graph_composition::runtime_node_id(&self.view_graph_id(), &node_id);
        let graph = self.graph.read();
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == node_id && matches!(node.config, NodeConfig::Provider(_)))
            .ok_or_else(|| {
                node_control::node_error(
                    ErrorCode::NodeNotFound,
                    &self.view_graph_id(),
                    node_id.as_str(),
                )
            })?;
        if !node.control.enabled {
            return Err(node_control::node_error(
                ErrorCode::NodeDisabled,
                &self.view_graph_id(),
                node_id.as_str(),
            ));
        }
        Ok(runtime_id)
    }
}

pub(super) fn processor_parameter_error(
    node_id: &str,
    parameter_id: u32,
    reason: &str,
) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::ProcessorUnavailable,
        parameters: serde_json::json!({
            "nodeId": node_id,
            "parameterId": parameter_id,
            "reason": reason,
        }),
    }
}

pub(super) fn processor_editor_error(node_id: &str, reason: &str) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::ProcessorUnavailable,
        parameters: serde_json::json!({ "nodeId": node_id, "reason": reason }),
    }
}

pub(super) fn persist_processor_parameter(
    provider: &mut mixrelay_audio_graph::ProviderNodeConfig,
    parameter_id: u32,
    value: f64,
) {
    // A non-object saved shape is replaced by the normalized object shape.
    let mut parameters = into_json_object(std::mem::take(&mut provider.parameters));
    parameters
        .entry("version")
        .or_insert_with(|| serde_json::json!(1));
    let values = parameters
        .entry("parameterValues")
        .or_insert(serde_json::Value::Null);
    let mut normalized = into_json_object(std::mem::take(values));
    normalized.insert(parameter_id.to_string(), serde_json::json!(value));
    *values = serde_json::Value::Object(normalized);
    provider.parameters = serde_json::Value::Object(parameters);
}

fn into_json_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}
