//! Injected capability resolution and provider compilation contracts.
use super::*;

struct UnavailableResolver;

impl NodeResolver for UnavailableResolver {
    fn resolve(
        &self,
        _node: &NodeSpec,
        _context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        Err(NodeResolutionError::CapabilityUnavailable {
            reason: "process tap permission denied".into(),
        })
    }
}

#[test]
fn custom_resolver_failure_is_reported_without_coupling_the_compiler() {
    let document = GraphDocument::new(1, vec![gain_node("capture")], vec![]);
    let error = GraphCompiler::new(UnavailableResolver)
        .compile(&document, context())
        .expect_err("unavailable capability should fail");

    assert!(matches!(
        error.issues(),
        [GraphIssue::NodeResolutionFailed { node_id, reason }]
            if node_id.as_str() == "capture" && reason.contains("permission denied")
    ));
}

struct TestProviderResolver;

impl NodeResolver for TestProviderResolver {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        if matches!(node.config, NodeConfig::Provider(_)) {
            Ok(ResolvedNode {
                inputs: vec![CompiledPort::new(
                    PortId::from(AUDIO_PORT_ID),
                    PortDirection::Input,
                    2,
                )],
                outputs: vec![CompiledPort::new(
                    PortId::from(AUDIO_PORT_ID),
                    PortDirection::Output,
                    2,
                )],
            })
        } else {
            BuiltInNodeResolver.resolve(node, context)
        }
    }
}

#[test]
fn provider_nodes_extend_the_graph_without_changing_the_compiler() {
    let provider_node = configured_node("compressor", provider("io.mixrelay.compressor"));
    let unavailable = BuiltInNodeResolver
        .resolve(&provider_node, &context())
        .expect_err("unregistered provider must be explicit");
    assert!(matches!(
        unavailable,
        NodeResolutionError::CapabilityUnavailable { reason }
            if reason.contains("io.mixrelay.compressor")
    ));

    let encoded = serde_json::to_value(&provider_node).expect("provider node should serialize");
    assert_eq!(encoded["type"], "provider");
    assert_eq!(encoded["config"]["parameters"]["thresholdDb"], -12.0);

    let document = GraphDocument::new(1, vec![provider_node], vec![]);
    let plan = GraphCompiler::new(TestProviderResolver)
        .compile(&document, context())
        .expect("registered provider should compile");
    assert_eq!(plan.nodes()[0].kind(), NodeKind::Provider);

    let built_in = TestProviderResolver
        .resolve(&gain_node("gain"), &context())
        .expect("provider resolver should compose with built-ins");
    assert_eq!(built_in.inputs.len(), 1);
}
