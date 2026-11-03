//! Stable graph diagnostics and machine-readable codes.
use super::*;

#[test]
fn every_issue_exposes_one_typed_consistent_code() {
    let endpoint = endpoint("node");
    let edge_id = EdgeId::from("edge");
    let issues = all_issue_variants(&endpoint, edge_id);
    let actual: Vec<_> = issues.iter().map(GraphIssue::code).collect();
    assert_eq!(actual, all_issue_codes());

    let encoded =
        serde_json::to_string(&GraphIssueCode::UnknownPort).expect("issue code should serialize");
    assert_eq!(encoded, "\"unknown_port\"");
}

fn all_issue_codes() -> Vec<GraphIssueCode> {
    vec![
        GraphIssueCode::UnsupportedSchemaVersion,
        GraphIssueCode::InvalidRenderFormat,
        GraphIssueCode::LimitExceeded,
        GraphIssueCode::InvalidNodeId,
        GraphIssueCode::DuplicateNodeId,
        GraphIssueCode::NodeResolutionFailed,
        GraphIssueCode::InvalidEdgeId,
        GraphIssueCode::DuplicateEdgeId,
        GraphIssueCode::UnknownNode,
        GraphIssueCode::UnknownPort,
        GraphIssueCode::InvalidPortDirection,
        GraphIssueCode::InputAlreadyConnected,
        GraphIssueCode::ChannelCountMismatch,
        GraphIssueCode::EmptyChannelMapping,
        GraphIssueCode::InvalidChannelRoute,
        GraphIssueCode::InvalidSelfConnection,
        GraphIssueCode::CycleDetected,
    ]
}

fn all_issue_variants(endpoint: &AudioEndpoint, edge_id: EdgeId) -> Vec<GraphIssue> {
    vec![
        GraphIssue::UnsupportedSchemaVersion {
            expected: 1,
            found: 2,
        },
        GraphIssue::InvalidRenderFormat {
            sample_rate: 0,
            block_frames: 0,
        },
        GraphIssue::LimitExceeded {
            resource: GraphResource::Nodes,
            limit: 0,
            actual: 1,
        },
        GraphIssue::InvalidNodeId,
        GraphIssue::DuplicateNodeId {
            node_id: NodeId::from("node"),
        },
        GraphIssue::NodeResolutionFailed {
            node_id: NodeId::from("node"),
            reason: "reason".into(),
        },
        GraphIssue::InvalidEdgeId,
        GraphIssue::DuplicateEdgeId {
            edge_id: edge_id.clone(),
        },
        GraphIssue::UnknownNode {
            edge_id: edge_id.clone(),
            node_id: NodeId::from("node"),
        },
        GraphIssue::UnknownPort {
            edge_id: edge_id.clone(),
            endpoint: endpoint.clone(),
        },
        GraphIssue::InvalidPortDirection {
            edge_id: edge_id.clone(),
            endpoint: endpoint.clone(),
            expected: PortDirection::Input,
            actual: PortDirection::Output,
        },
        GraphIssue::InputAlreadyConnected {
            edge_id: edge_id.clone(),
            endpoint: endpoint.clone(),
        },
        GraphIssue::ChannelCountMismatch {
            edge_id: edge_id.clone(),
            source_channels: 1,
            destination_channels: 2,
        },
        GraphIssue::EmptyChannelMapping {
            edge_id: edge_id.clone(),
        },
        GraphIssue::InvalidChannelRoute {
            edge_id,
            route: ChannelRoute {
                source_channel: 2,
                destination_channel: 2,
            },
            source_channels: 2,
            destination_channels: 2,
        },
        GraphIssue::InvalidSelfConnection {
            edge_id: EdgeId::from("edge"),
            node_id: NodeId::from("node"),
        },
        GraphIssue::CycleDetected {
            node_ids: vec![NodeId::from("node")],
        },
    ]
}
