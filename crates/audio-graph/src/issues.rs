//! Graph compilation diagnostics.

use crate::model::{AudioEndpoint, ChannelRoute, EdgeId, NodeId, PortDirection};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum GraphIssue {
    UnsupportedSchemaVersion {
        expected: u32,
        found: u32,
    },
    InvalidRenderFormat {
        sample_rate: u32,
        block_frames: u32,
    },
    LimitExceeded {
        resource: GraphResource,
        limit: usize,
        actual: usize,
    },
    InvalidNodeId,
    DuplicateNodeId {
        node_id: NodeId,
    },
    NodeResolutionFailed {
        node_id: NodeId,
        reason: String,
    },
    InvalidEdgeId,
    DuplicateEdgeId {
        edge_id: EdgeId,
    },
    UnknownNode {
        edge_id: EdgeId,
        node_id: NodeId,
    },
    UnknownPort {
        edge_id: EdgeId,
        endpoint: AudioEndpoint,
    },
    InvalidPortDirection {
        edge_id: EdgeId,
        endpoint: AudioEndpoint,
        expected: PortDirection,
        actual: PortDirection,
    },
    InputAlreadyConnected {
        edge_id: EdgeId,
        endpoint: AudioEndpoint,
    },
    ChannelCountMismatch {
        edge_id: EdgeId,
        source_channels: u16,
        destination_channels: u16,
    },
    EmptyChannelMapping {
        edge_id: EdgeId,
    },
    InvalidChannelRoute {
        edge_id: EdgeId,
        route: ChannelRoute,
        source_channels: u16,
        destination_channels: u16,
    },
    InvalidSelfConnection {
        edge_id: EdgeId,
        node_id: NodeId,
    },
    DuplicateVirtualEndpointId {
        endpoint_id: String,
        first_node_id: NodeId,
        duplicate_node_id: NodeId,
    },
    CycleDetected {
        node_ids: Vec<NodeId>,
    },
}

impl GraphIssue {
    #[must_use]
    pub const fn code(&self) -> GraphIssueCode {
        match self {
            Self::UnsupportedSchemaVersion { .. } => GraphIssueCode::UnsupportedSchemaVersion,
            Self::InvalidRenderFormat { .. } => GraphIssueCode::InvalidRenderFormat,
            Self::LimitExceeded { .. } => GraphIssueCode::LimitExceeded,
            Self::InvalidNodeId => GraphIssueCode::InvalidNodeId,
            Self::DuplicateNodeId { .. } => GraphIssueCode::DuplicateNodeId,
            Self::NodeResolutionFailed { .. } => GraphIssueCode::NodeResolutionFailed,
            Self::InvalidEdgeId => GraphIssueCode::InvalidEdgeId,
            Self::DuplicateEdgeId { .. } => GraphIssueCode::DuplicateEdgeId,
            Self::UnknownNode { .. } => GraphIssueCode::UnknownNode,
            Self::UnknownPort { .. } => GraphIssueCode::UnknownPort,
            Self::InvalidPortDirection { .. } => GraphIssueCode::InvalidPortDirection,
            Self::InputAlreadyConnected { .. } => GraphIssueCode::InputAlreadyConnected,
            Self::ChannelCountMismatch { .. } => GraphIssueCode::ChannelCountMismatch,
            Self::EmptyChannelMapping { .. } => GraphIssueCode::EmptyChannelMapping,
            Self::InvalidChannelRoute { .. } => GraphIssueCode::InvalidChannelRoute,
            Self::InvalidSelfConnection { .. } => GraphIssueCode::InvalidSelfConnection,
            Self::DuplicateVirtualEndpointId { .. } => GraphIssueCode::DuplicateVirtualEndpointId,
            Self::CycleDetected { .. } => GraphIssueCode::CycleDetected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphIssueCode {
    UnsupportedSchemaVersion,
    InvalidRenderFormat,
    LimitExceeded,
    InvalidNodeId,
    DuplicateNodeId,
    NodeResolutionFailed,
    InvalidEdgeId,
    DuplicateEdgeId,
    UnknownNode,
    UnknownPort,
    InvalidPortDirection,
    InputAlreadyConnected,
    ChannelCountMismatch,
    EmptyChannelMapping,
    InvalidChannelRoute,
    InvalidSelfConnection,
    DuplicateVirtualEndpointId,
    CycleDetected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphResource {
    Nodes,
    Edges,
    PlanBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    issues: Vec<GraphIssue>,
}

impl CompileError {
    pub(crate) fn new(issues: Vec<GraphIssue>) -> Self {
        Self { issues }
    }

    #[must_use]
    pub fn issues(&self) -> &[GraphIssue] {
        &self.issues
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "graph compilation failed with {} issue(s)",
            self.issues.len()
        )
    }
}

impl std::error::Error for CompileError {}
