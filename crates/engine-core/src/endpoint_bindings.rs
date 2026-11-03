//! Validates virtual endpoint identities against prepared DSP input and output bindings.

use crate::{
    BTreeSet, ErrorCode, ExternalBinding, FROM_OS_PORT_ID, GraphDocument, NodeConfig, NodeId,
    ProtocolError, RenderPlan, TO_OS_PORT_ID, VirtualEndpointCapabilities, VirtualEndpointId,
    VirtualEndpointSpec,
};

pub(super) fn virtual_endpoint_specs(
    document: &GraphDocument,
    capabilities: &VirtualEndpointCapabilities,
) -> Result<Vec<(NodeId, VirtualEndpointSpec)>, ProtocolError> {
    let mut endpoint_ids = BTreeSet::new();
    let mut endpoints = Vec::new();
    for node in &document.nodes {
        let NodeConfig::VirtualEndpointTransport(endpoint) = &node.config else {
            continue;
        };
        if !endpoint_ids.insert(endpoint.id.clone()) {
            return Err(ProtocolError {
                code: ErrorCode::GraphBindingUnsupported,
                parameters: serde_json::json!({
                    "endpointId": endpoint.id.as_str(),
                    "reason": "virtual endpoint IDs must be unique",
                }),
            });
        }
        endpoint
            .validate(capabilities)
            .map_err(|error| ProtocolError {
                code: ErrorCode::GraphBindingUnsupported,
                parameters: serde_json::json!({
                    "endpointId": endpoint.id.as_str(),
                    "reason": error.to_string(),
                }),
            })?;
        endpoints.push((node.id.clone(), endpoint.clone()));
    }
    Ok(endpoints)
}

pub(super) fn validate_endpoint_bindings(
    plan: &RenderPlan,
    endpoints: &[(NodeId, VirtualEndpointSpec)],
) -> Result<(), ProtocolError> {
    let mut missing = Vec::new();
    for (node_id, endpoint) in endpoints {
        if let Some(stream) = endpoint.capture
            && !binding_exists(
                plan.source_bindings(),
                node_id,
                &endpoint.id,
                FROM_OS_PORT_ID,
                stream.channels,
            )
        {
            missing.push(serde_json::json!({
                "nodeId": node_id.as_str(),
                "portId": FROM_OS_PORT_ID,
                "channels": stream.channels,
            }));
        }
        if let Some(stream) = endpoint.render
            && !binding_exists(
                plan.sink_bindings(),
                node_id,
                &endpoint.id,
                TO_OS_PORT_ID,
                stream.channels,
            )
        {
            missing.push(serde_json::json!({
                "nodeId": node_id.as_str(),
                "portId": TO_OS_PORT_ID,
                "channels": stream.channels,
            }));
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(ProtocolError {
        code: ErrorCode::GraphBindingUnsupported,
        parameters: serde_json::json!({
            "missing": missing,
            "sources": binding_values(plan.source_bindings()),
            "sinks": binding_values(plan.sink_bindings()),
        }),
    })
}

fn binding_exists(
    bindings: &[ExternalBinding],
    node_id: &NodeId,
    endpoint_id: &VirtualEndpointId,
    port_id: &str,
    channels: u16,
) -> bool {
    bindings.iter().any(|binding| {
        binding.node_id() == node_id
            && binding.virtual_endpoint_id() == Some(endpoint_id)
            && binding.port_id().as_str() == port_id
            && binding.channels() == channels
    })
}

fn binding_values(bindings: &[ExternalBinding]) -> Vec<serde_json::Value> {
    bindings
        .iter()
        .map(|binding| {
            serde_json::json!({
                "nodeId": binding.node_id().as_str(),
                "portId": binding.port_id().as_str(),
                "channels": binding.channels(),
            })
        })
        .collect()
}
