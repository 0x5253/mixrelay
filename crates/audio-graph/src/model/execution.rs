//! Immutable compiled execution topology and its owned-memory accounting.

use super::{
    AudioEndpoint, ChannelRoute, EdgeId, NodeConfig, NodeControl, NodeId, NodeKind, PortDirection,
    PortId, RenderFormat,
};
use std::mem::size_of_val;

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionPlan {
    revision: u64,
    format: RenderFormat,
    nodes: Box<[CompiledNode]>,
    edges: Box<[CompiledEdge]>,
}

impl ExecutionPlan {
    pub(crate) fn new(
        revision: u64,
        format: RenderFormat,
        nodes: Vec<CompiledNode>,
        edges: Vec<CompiledEdge>,
    ) -> Self {
        Self {
            revision,
            format,
            nodes: nodes.into_boxed_slice(),
            edges: edges.into_boxed_slice(),
        }
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn format(&self) -> RenderFormat {
        self.format
    }

    #[must_use]
    pub fn nodes(&self) -> &[CompiledNode] {
        &self.nodes
    }

    #[must_use]
    pub fn edges(&self) -> &[CompiledEdge] {
        &self.edges
    }

    pub(crate) fn owned_memory_bytes(&self) -> usize {
        let heap_capacity_bytes = size_of_val(self.nodes.as_ref())
            + size_of_val(self.edges.as_ref())
            + self
                .nodes
                .iter()
                .map(CompiledNode::nested_heap_capacity_bytes)
                .sum::<usize>()
            + self
                .edges
                .iter()
                .map(CompiledEdge::nested_heap_capacity_bytes)
                .sum::<usize>();
        size_of_val(self) + heap_capacity_bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledNode {
    id: NodeId,
    config: NodeConfig,
    control: NodeControl,
    inputs: Box<[CompiledPort]>,
    outputs: Box<[CompiledPort]>,
}

impl CompiledNode {
    pub(crate) fn new(
        id: NodeId,
        config: NodeConfig,
        control: NodeControl,
        inputs: Vec<CompiledPort>,
        outputs: Vec<CompiledPort>,
    ) -> Self {
        Self {
            id,
            config,
            control,
            inputs: inputs.into_boxed_slice(),
            outputs: outputs.into_boxed_slice(),
        }
    }

    #[must_use]
    pub fn id(&self) -> &NodeId {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        self.config.kind()
    }

    #[must_use]
    pub const fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// Returns the controls captured when this immutable execution plan was compiled.
    #[must_use]
    pub const fn control(&self) -> NodeControl {
        self.control
    }

    #[must_use]
    pub fn inputs(&self) -> &[CompiledPort] {
        &self.inputs
    }

    #[must_use]
    pub fn outputs(&self) -> &[CompiledPort] {
        &self.outputs
    }

    fn nested_heap_capacity_bytes(&self) -> usize {
        let encoded_config_bytes = match &self.config {
            NodeConfig::VirtualEndpointTransport(spec) => spec.owned_heap_capacity_bytes(),
            NodeConfig::DeviceBus { .. } => 0,
            _ => serde_json::to_vec(&self.config).map_or(0, |encoded| encoded.len()),
        };
        self.id.0.capacity()
            + encoded_config_bytes
            + size_of_val(self.inputs.as_ref())
            + size_of_val(self.outputs.as_ref())
            + self
                .inputs
                .iter()
                .chain(&self.outputs)
                .map(|port| port.id.0.capacity())
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPort {
    id: PortId,
    direction: PortDirection,
    channels: u16,
}

impl CompiledPort {
    #[must_use]
    pub const fn new(id: PortId, direction: PortDirection, channels: u16) -> Self {
        Self {
            id,
            direction,
            channels,
        }
    }

    #[must_use]
    pub fn id(&self) -> &PortId {
        &self.id
    }

    #[must_use]
    pub const fn direction(&self) -> PortDirection {
        self.direction
    }

    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledEdge {
    id: EdgeId,
    source: AudioEndpoint,
    destination: AudioEndpoint,
    routes: Box<[ChannelRoute]>,
}

impl CompiledEdge {
    pub(crate) fn new(
        id: EdgeId,
        source: AudioEndpoint,
        destination: AudioEndpoint,
        routes: Vec<ChannelRoute>,
    ) -> Self {
        Self {
            id,
            source,
            destination,
            routes: routes.into_boxed_slice(),
        }
    }

    #[must_use]
    pub fn id(&self) -> &EdgeId {
        &self.id
    }

    #[must_use]
    pub fn source(&self) -> &AudioEndpoint {
        &self.source
    }

    #[must_use]
    pub fn destination(&self) -> &AudioEndpoint {
        &self.destination
    }

    #[must_use]
    pub fn routes(&self) -> &[ChannelRoute] {
        &self.routes
    }

    fn nested_heap_capacity_bytes(&self) -> usize {
        self.id.0.capacity()
            + self.source.node_id.0.capacity()
            + self.source.port_id.0.capacity()
            + self.destination.node_id.0.capacity()
            + self.destination.port_id.0.capacity()
            + size_of_val(self.routes.as_ref())
    }
}
