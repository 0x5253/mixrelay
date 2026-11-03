//! Resolves graph nodes, validates connections and orders the execution plan.

use crate::builtin::{BuiltInNodeResolver, resolve_dormant_provider};
use crate::issues::{CompileError, GraphIssue, GraphResource};
use crate::model::{
    AudioEndpoint, CURRENT_GRAPH_SCHEMA_VERSION, ChannelMapping, ChannelRoute, CompiledEdge,
    CompiledNode, CompiledPort, EdgeId, ExecutionPlan, FROM_OS_PORT_ID, GraphDocument, NodeConfig,
    NodeId, NodeSpec, PortDirection, TO_OS_PORT_ID,
};
use crate::resolution::{CompileContext, NodeResolver};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct GraphCompiler {
    resolver: Arc<dyn NodeResolver>,
}

impl Default for GraphCompiler {
    fn default() -> Self {
        Self::new(BuiltInNodeResolver)
    }
}

impl GraphCompiler {
    #[must_use]
    pub fn new(resolver: impl NodeResolver + 'static) -> Self {
        Self {
            resolver: Arc::new(resolver),
        }
    }

    /// Compiles a persisted graph into an immutable runtime plan.
    ///
    /// # Errors
    ///
    /// Returns [`CompileError`] containing every validation issue that can be
    /// found without making assumptions about an invalid graph.
    pub fn compile(
        &self,
        document: &GraphDocument,
        context: CompileContext,
    ) -> Result<ExecutionPlan, CompileError> {
        let mut issues = validate_document(document, &context);
        let mut nodes = self.resolve_nodes(document, &context, &mut issues);
        let (compiled_edges, dependencies) = compile_edges(document, &nodes, &mut issues);

        if !issues.is_empty() {
            return Err(CompileError::new(issues));
        }

        let ordered_ids = topological_order(&nodes, &dependencies).map_err(|nodes| {
            CompileError::new(vec![GraphIssue::CycleDetected { node_ids: nodes }])
        })?;
        let compiled_nodes = ordered_ids
            .into_iter()
            .filter_map(|node_id| nodes.remove(&node_id))
            .collect();

        let plan = ExecutionPlan::new(
            document.revision,
            context.format,
            compiled_nodes,
            compiled_edges,
        );
        let actual_plan_bytes = plan.owned_memory_bytes();
        if actual_plan_bytes > context.limits.max_plan_bytes {
            return Err(CompileError::new(vec![GraphIssue::LimitExceeded {
                resource: GraphResource::PlanBytes,
                limit: context.limits.max_plan_bytes,
                actual: actual_plan_bytes,
            }]));
        }
        Ok(plan)
    }

    fn resolve_nodes(
        &self,
        document: &GraphDocument,
        context: &CompileContext,
        issues: &mut Vec<GraphIssue>,
    ) -> BTreeMap<NodeId, CompiledNode> {
        let mut nodes = BTreeMap::new();
        for node in &document.nodes {
            if node.id.as_str().trim().is_empty() {
                issues.push(GraphIssue::InvalidNodeId);
            } else if nodes.contains_key(&node.id) {
                issues.push(GraphIssue::DuplicateNodeId {
                    node_id: node.id.clone(),
                });
            } else {
                self.resolve_node(node, context, &mut nodes, issues);
            }
        }
        nodes
    }

    fn resolve_node(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
        nodes: &mut BTreeMap<NodeId, CompiledNode>,
        issues: &mut Vec<GraphIssue>,
    ) {
        match self
            .resolver
            .resolve(node, context)
            .or_else(|error| resolve_dormant_provider(node, context, error))
        {
            Ok(resolved) => {
                nodes.insert(
                    node.id.clone(),
                    CompiledNode::new(
                        node.id.clone(),
                        node.config.clone(),
                        node.control,
                        resolved.inputs,
                        resolved.outputs,
                    ),
                );
            }
            Err(error) => issues.push(GraphIssue::NodeResolutionFailed {
                node_id: node.id.clone(),
                reason: error.to_string(),
            }),
        }
    }
}

fn validate_document(document: &GraphDocument, context: &CompileContext) -> Vec<GraphIssue> {
    let mut issues = Vec::new();
    if document.schema_version != CURRENT_GRAPH_SCHEMA_VERSION {
        issues.push(GraphIssue::UnsupportedSchemaVersion {
            expected: CURRENT_GRAPH_SCHEMA_VERSION,
            found: document.schema_version,
        });
    }
    if context.format.sample_rate == 0
        || context.format.sample_rate > context.limits.max_sample_rate
        || context.format.block_frames == 0
        || context.format.block_frames > context.limits.max_block_frames
    {
        issues.push(GraphIssue::InvalidRenderFormat {
            sample_rate: context.format.sample_rate,
            block_frames: context.format.block_frames,
        });
    }
    append_limit_issue(
        &mut issues,
        GraphResource::Nodes,
        context.limits.max_nodes,
        document.nodes.len(),
    );
    append_limit_issue(
        &mut issues,
        GraphResource::Edges,
        context.limits.max_edges,
        document.edges.len(),
    );
    let mut endpoint_nodes = BTreeMap::<&str, &NodeId>::new();
    for node in &document.nodes {
        let NodeConfig::VirtualEndpointTransport(endpoint) = &node.config else {
            continue;
        };
        if let Some(first_node_id) = endpoint_nodes.get(endpoint.id.as_str()) {
            issues.push(GraphIssue::DuplicateVirtualEndpointId {
                endpoint_id: endpoint.id.as_str().to_owned(),
                first_node_id: (*first_node_id).clone(),
                duplicate_node_id: node.id.clone(),
            });
        } else {
            endpoint_nodes.insert(endpoint.id.as_str(), &node.id);
        }
    }
    issues
}

fn append_limit_issue(
    issues: &mut Vec<GraphIssue>,
    resource: GraphResource,
    limit: usize,
    actual: usize,
) {
    if actual > limit {
        issues.push(GraphIssue::LimitExceeded {
            resource,
            limit,
            actual,
        });
    }
}

fn compile_edges(
    document: &GraphDocument,
    nodes: &BTreeMap<NodeId, CompiledNode>,
    issues: &mut Vec<GraphIssue>,
) -> (Vec<CompiledEdge>, BTreeMap<NodeId, BTreeSet<NodeId>>) {
    let mut state = EdgeCompilationState::new(nodes);
    for edge in &document.edges {
        state.compile(edge, issues);
    }
    (state.edges, state.dependencies)
}

struct EdgeCompilationState<'a> {
    nodes: &'a BTreeMap<NodeId, CompiledNode>,
    edge_ids: BTreeSet<EdgeId>,
    edges: Vec<CompiledEdge>,
    dependencies: BTreeMap<NodeId, BTreeSet<NodeId>>,
}

impl<'a> EdgeCompilationState<'a> {
    fn new(nodes: &'a BTreeMap<NodeId, CompiledNode>) -> Self {
        Self {
            nodes,
            edge_ids: BTreeSet::new(),
            edges: Vec::new(),
            dependencies: nodes
                .keys()
                .cloned()
                .map(|node_id| (node_id, BTreeSet::new()))
                .collect(),
        }
    }

    fn compile(&mut self, edge: &crate::model::EdgeSpec, issues: &mut Vec<GraphIssue>) {
        if !self.validate_id(edge, issues) {
            return;
        }
        let issue_count = issues.len();
        let source = resolve_port(
            self.nodes,
            &edge.id,
            &edge.source,
            PortDirection::Output,
            issues,
        );
        let destination = resolve_port(
            self.nodes,
            &edge.id,
            &edge.destination,
            PortDirection::Input,
            issues,
        );
        self.validate_self_connection(edge, issues);
        let routes = Self::resolve_routes(edge, source, destination, issues);
        if issues.len() == issue_count
            && let Some(routes) = routes
        {
            self.push(edge, routes);
        }
    }

    fn validate_self_connection(
        &self,
        edge: &crate::model::EdgeSpec,
        issues: &mut Vec<GraphIssue>,
    ) {
        if edge.source.node_id != edge.destination.node_id {
            return;
        }
        let is_native_pass_through = self.nodes.get(&edge.source.node_id).is_some_and(|node| {
            matches!(
                node.config(),
                NodeConfig::VirtualEndpoint(_) | NodeConfig::VirtualEndpointTransport(_)
            ) && edge.source.port_id.as_str() == FROM_OS_PORT_ID
                && edge.destination.port_id.as_str() == TO_OS_PORT_ID
        });
        if !is_native_pass_through {
            issues.push(GraphIssue::InvalidSelfConnection {
                edge_id: edge.id.clone(),
                node_id: edge.source.node_id.clone(),
            });
        }
    }

    fn validate_id(&mut self, edge: &crate::model::EdgeSpec, issues: &mut Vec<GraphIssue>) -> bool {
        if edge.id.as_str().trim().is_empty() {
            issues.push(GraphIssue::InvalidEdgeId);
            false
        } else if !self.edge_ids.insert(edge.id.clone()) {
            issues.push(GraphIssue::DuplicateEdgeId {
                edge_id: edge.id.clone(),
            });
            false
        } else {
            true
        }
    }

    fn resolve_routes(
        edge: &crate::model::EdgeSpec,
        source: Option<&CompiledPort>,
        destination: Option<&CompiledPort>,
        issues: &mut Vec<GraphIssue>,
    ) -> Option<Vec<ChannelRoute>> {
        let (Some(source), Some(destination)) = (source, destination) else {
            return None;
        };
        compile_channel_mapping(
            &edge.id,
            source.channels(),
            destination.channels(),
            &edge.channel_mapping,
            issues,
        )
    }

    fn push(&mut self, edge: &crate::model::EdgeSpec, routes: Vec<ChannelRoute>) {
        if edge.source.node_id != edge.destination.node_id && self.edge_orders_processing(edge) {
            self.dependencies
                .entry(edge.source.node_id.clone())
                .or_default()
                .insert(edge.destination.node_id.clone());
        }
        self.edges.push(CompiledEdge::new(
            edge.id.clone(),
            edge.source.clone(),
            edge.destination.clone(),
            routes,
        ));
    }

    fn edge_orders_processing(&self, edge: &crate::model::EdgeSpec) -> bool {
        let source_is_external = self.nodes.get(&edge.source.node_id).is_some_and(|node| {
            matches!(
                node.config(),
                NodeConfig::ApplicationCapture(_)
                    | NodeConfig::AudioDeviceInput(_)
                    | NodeConfig::VirtualEndpointTransport(_)
                    | NodeConfig::VirtualEndpoint(_)
                    | NodeConfig::DeviceBus { .. }
            )
        });
        let destination_is_external =
            self.nodes
                .get(&edge.destination.node_id)
                .is_some_and(|node| {
                    matches!(
                        node.config(),
                        NodeConfig::AudioDeviceOutput(_)
                            | NodeConfig::Recorder { .. }
                            | NodeConfig::VirtualEndpointTransport(_)
                            | NodeConfig::VirtualEndpoint(_)
                            | NodeConfig::DeviceBus { .. }
                    )
                });
        !source_is_external && !destination_is_external
    }
}

fn resolve_port<'a>(
    nodes: &'a BTreeMap<NodeId, CompiledNode>,
    edge_id: &EdgeId,
    endpoint: &AudioEndpoint,
    expected_direction: PortDirection,
    issues: &mut Vec<GraphIssue>,
) -> Option<&'a CompiledPort> {
    let Some(node) = nodes.get(&endpoint.node_id) else {
        issues.push(GraphIssue::UnknownNode {
            edge_id: edge_id.clone(),
            node_id: endpoint.node_id.clone(),
        });
        return None;
    };

    let (expected_ports, opposite_ports) = match expected_direction {
        PortDirection::Input => (node.inputs(), node.outputs()),
        PortDirection::Output => (node.outputs(), node.inputs()),
    };
    if let Some(port) = expected_ports
        .iter()
        .find(|port| port.id() == &endpoint.port_id)
    {
        return Some(port);
    }
    if let Some(port) = opposite_ports
        .iter()
        .find(|port| port.id() == &endpoint.port_id)
    {
        issues.push(GraphIssue::InvalidPortDirection {
            edge_id: edge_id.clone(),
            endpoint: endpoint.clone(),
            expected: expected_direction,
            actual: port.direction(),
        });
        return None;
    }
    issues.push(GraphIssue::UnknownPort {
        edge_id: edge_id.clone(),
        endpoint: endpoint.clone(),
    });
    None
}

fn compile_channel_mapping(
    edge_id: &EdgeId,
    source_channels: u16,
    destination_channels: u16,
    mapping: &ChannelMapping,
    issues: &mut Vec<GraphIssue>,
) -> Option<Vec<ChannelRoute>> {
    match mapping {
        ChannelMapping::Auto if source_channels == destination_channels => Some(
            (0..source_channels)
                .map(|channel| ChannelRoute {
                    source_channel: channel,
                    destination_channel: channel,
                })
                .collect(),
        ),
        ChannelMapping::Auto => {
            issues.push(GraphIssue::ChannelCountMismatch {
                edge_id: edge_id.clone(),
                source_channels,
                destination_channels,
            });
            None
        }
        ChannelMapping::Explicit { routes } => {
            if routes.is_empty() {
                issues.push(GraphIssue::EmptyChannelMapping {
                    edge_id: edge_id.clone(),
                });
                return None;
            }

            let mut unique_routes = BTreeSet::new();
            for route in routes {
                if route.source_channel >= source_channels
                    || route.destination_channel >= destination_channels
                    || !unique_routes.insert((route.source_channel, route.destination_channel))
                {
                    issues.push(GraphIssue::InvalidChannelRoute {
                        edge_id: edge_id.clone(),
                        route: *route,
                        source_channels,
                        destination_channels,
                    });
                }
            }
            Some(routes.clone())
        }
    }
}

fn topological_order(
    nodes: &BTreeMap<NodeId, CompiledNode>,
    dependencies: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> Result<Vec<NodeId>, Vec<NodeId>> {
    let mut indegree: BTreeMap<NodeId, usize> =
        nodes.keys().cloned().map(|node_id| (node_id, 0)).collect();
    for destination in dependencies.values().flatten() {
        *indegree.entry(destination.clone()).or_default() += 1;
    }

    let mut ready: BTreeSet<NodeId> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(node_id, _)| node_id.clone())
        .collect();
    let mut ordered = Vec::with_capacity(nodes.len());

    while let Some(node_id) = ready.pop_first() {
        ordered.push(node_id.clone());
        for destination in dependencies.get(&node_id).into_iter().flatten() {
            // The counting pass above inserted every dependency destination.
            let Some(count) = indegree.get_mut(destination) else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                ready.insert(destination.clone());
            }
        }
    }

    if ordered.len() == nodes.len() {
        Ok(ordered)
    } else {
        Err(indegree
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(node_id, _)| node_id)
            .collect())
    }
}
