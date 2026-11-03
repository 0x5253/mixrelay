use super::{LatencyNode, LatencyPlanError, LatencyRoute, route_delays};

fn route(source: usize, destination: usize) -> LatencyRoute {
    LatencyRoute {
        source,
        destination,
    }
}

#[test]
fn aligns_diamond_paths_at_the_processing_merge() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(3)),
        LatencyNode::Processing(Some(9)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Sink,
    ];
    let routes = [
        route(0, 1),
        route(0, 2),
        route(1, 3),
        route(2, 3),
        route(3, 4),
    ];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0, 6, 0, 0]));
}

#[test]
fn aligns_dry_and_wet_paths_at_the_sink_merge() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(3)),
        LatencyNode::Processing(Some(8)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 1), route(0, 2), route(1, 3), route(2, 3)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0, 5, 0]));
}

#[test]
fn propagates_nested_merge_arrivals() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(1)),
        LatencyNode::Processing(Some(3)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Processing(Some(4)),
        LatencyNode::Sink,
    ];
    let routes = [
        route(0, 1),
        route(0, 2),
        route(1, 3),
        route(2, 3),
        route(3, 4),
        route(2, 4),
        route(4, 5),
        route(1, 5),
    ];

    assert_eq!(
        route_delays(&nodes, &routes),
        Ok(vec![0, 0, 2, 0, 0, 2, 0, 8])
    );
}

#[test]
fn unknown_merge_is_not_treated_as_known_zero() {
    let known_nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Source(Some(5)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Sink,
    ];
    let unknown_nodes = [
        LatencyNode::Source(None),
        LatencyNode::Source(Some(5)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 2), route(1, 2), route(2, 3)];

    assert_eq!(route_delays(&known_nodes, &routes), Ok(vec![5, 0, 0]));
    assert_eq!(route_delays(&unknown_nodes, &routes), Ok(vec![0, 0, 0]));
}

#[test]
fn unknown_merge_arrival_propagates_to_downstream_processing() {
    let nodes = [
        LatencyNode::Source(None),
        LatencyNode::Source(Some(5)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Source(Some(10)),
        LatencyNode::Processing(Some(3)),
        LatencyNode::Sink,
    ];
    let routes = [
        route(0, 2),
        route(1, 2),
        route(2, 4),
        route(3, 4),
        route(4, 5),
    ];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0, 0, 0, 0]));
}

#[test]
fn unknown_intrinsic_latency_keeps_downstream_arrival_unknown() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Source(Some(5)),
        LatencyNode::Processing(None),
        LatencyNode::Source(Some(10)),
        LatencyNode::Processing(Some(0)),
        LatencyNode::Sink,
    ];
    let routes = [
        route(0, 2),
        route(1, 2),
        route(2, 4),
        route(3, 4),
        route(4, 5),
    ];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![5, 0, 0, 0, 0]));
}

#[test]
fn unknown_terminal_merge_does_not_align_known_subset() {
    let nodes = [
        LatencyNode::Source(None),
        LatencyNode::Source(Some(8)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 2), route(1, 2)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0]));
}

#[test]
fn computes_processing_order_when_nodes_are_shuffled() {
    let nodes = [
        LatencyNode::Sink,
        LatencyNode::Processing(Some(0)),
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(4)),
    ];
    let routes = [route(2, 1), route(2, 3), route(1, 0), route(3, 0)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0, 4, 0]));
}

#[test]
fn treats_all_processing_inputs_as_one_merge() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Source(Some(7)),
        LatencyNode::Processing(Some(1)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 2), route(1, 2), route(2, 3)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![7, 0, 0]));
}

#[test]
fn unconnected_processing_input_starts_at_zero_arrival() {
    let nodes = [
        LatencyNode::Processing(Some(7)),
        LatencyNode::Source(Some(0)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 2), route(1, 2)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 7]));
}

#[test]
fn aligns_terminal_sink_merge_after_processing_arrivals_are_known() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 2), route(0, 1), route(1, 2)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![2, 0, 0]));
}

#[test]
fn source_incoming_routes_do_not_change_its_causal_output() {
    let nodes = [
        LatencyNode::Source(Some(7)),
        LatencyNode::Processing(Some(2)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 1), route(1, 0), route(0, 2)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0, 0]));
}

#[test]
fn permits_duplicate_routes_and_preserves_route_order() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(0)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 1), route(0, 1), route(1, 2)];

    assert_eq!(route_delays(&nodes, &routes), Ok(vec![0, 0, 0]));
}

#[test]
fn rejects_processing_cycles() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(0)),
        LatencyNode::Processing(Some(0)),
        LatencyNode::Sink,
    ];
    let routes = [route(0, 1), route(1, 2), route(2, 1), route(2, 3)];

    assert_eq!(
        route_delays(&nodes, &routes),
        Err(LatencyPlanError::ProcessingCycle)
    );
}

#[test]
fn rejects_out_of_range_route_endpoints() {
    let nodes = [LatencyNode::Source(Some(0))];
    let invalid_source = [route(1, 0)];
    let invalid_destination = [route(0, 1)];

    assert_eq!(
        route_delays(&nodes, &invalid_source),
        Err(LatencyPlanError::InvalidSourceIndex {
            route_index: 0,
            source_index: 1,
            node_count: 1,
        })
    );
    assert_eq!(
        route_delays(&nodes, &invalid_destination),
        Err(LatencyPlanError::InvalidDestinationIndex {
            route_index: 0,
            destination_index: 1,
            node_count: 1,
        })
    );
}

#[test]
fn rejects_a_sink_as_route_source() {
    let nodes = [LatencyNode::Sink, LatencyNode::Source(Some(0))];
    let routes = [route(0, 1)];

    assert_eq!(
        route_delays(&nodes, &routes),
        Err(LatencyPlanError::SinkUsedAsSource {
            route_index: 0,
            source_index: 0,
        })
    );
}

#[test]
fn reports_cumulative_latency_overflow() {
    let nodes = [
        LatencyNode::Source(Some(u32::MAX)),
        LatencyNode::Processing(Some(1)),
    ];
    let routes = [route(0, 1)];

    assert_eq!(
        route_delays(&nodes, &routes),
        Err(LatencyPlanError::LatencyOverflow { node_index: 1 })
    );
}

#[test]
fn retains_terminal_arrivals_without_overwriting_causal_roots() {
    let nodes = [
        LatencyNode::Source(Some(0)),
        LatencyNode::Processing(Some(12)),
        LatencyNode::Source(Some(3)),
        LatencyNode::Sink,
        LatencyNode::Sink,
    ];
    let routes = [route(0, 1), route(1, 2), route(2, 3)];
    let plan = super::plan_arrivals(&nodes, &routes).unwrap();
    assert_eq!(
        plan.terminal_arrivals,
        vec![None, None, Some(12), Some(3), None]
    );
    let plan = super::plan_arrivals(
        &[LatencyNode::Source(None), LatencyNode::Sink],
        &[route(0, 1)],
    )
    .unwrap();
    assert_eq!(plan.terminal_arrivals, vec![None, None]);
}
