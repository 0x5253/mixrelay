use std::collections::VecDeque;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatencyNode {
    /// A causal input root whose output arrival is fixed independently of incoming routes.
    Source(Option<u32>),
    /// A processing node with optional known total latency, including hosted transport delay.
    Processing(Option<u32>),
    /// A terminal node that only merges incoming routes.
    Sink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LatencyRoute {
    pub source: usize,
    pub destination: usize,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatencyPlanError {
    #[error("route {route_index} source index {source_index} is outside {node_count} nodes")]
    InvalidSourceIndex {
        route_index: usize,
        source_index: usize,
        node_count: usize,
    },
    #[error(
        "route {route_index} destination index {destination_index} is outside {node_count} nodes"
    )]
    InvalidDestinationIndex {
        route_index: usize,
        destination_index: usize,
        node_count: usize,
    },
    #[error("route {route_index} uses sink node {source_index} as a source")]
    SinkUsedAsSource {
        route_index: usize,
        source_index: usize,
    },
    #[error("processing nodes contain a directed cycle")]
    ProcessingCycle,
    #[error("latency accumulation overflowed at processing node {node_index}")]
    LatencyOverflow { node_index: usize },
}

#[derive(Debug)]
struct RouteGraph {
    incoming: Vec<Vec<usize>>,
    processing_outgoing: Vec<Vec<usize>>,
    processing_indegree: Vec<usize>,
    processing_count: usize,
}

impl RouteGraph {
    fn new(nodes: &[LatencyNode], routes: &[LatencyRoute]) -> Result<Self, LatencyPlanError> {
        let mut graph = Self {
            incoming: std::iter::repeat_with(Vec::new).take(nodes.len()).collect(),
            processing_outgoing: std::iter::repeat_with(Vec::new).take(nodes.len()).collect(),
            processing_indegree: vec![0; nodes.len()],
            processing_count: nodes
                .iter()
                .filter(|node| matches!(node, LatencyNode::Processing(_)))
                .count(),
        };

        for (route_index, route) in routes.iter().enumerate() {
            graph.add_route(nodes, route_index, *route)?;
        }
        Ok(graph)
    }

    fn add_route(
        &mut self,
        nodes: &[LatencyNode],
        route_index: usize,
        route: LatencyRoute,
    ) -> Result<(), LatencyPlanError> {
        if route.source >= nodes.len() {
            return Err(LatencyPlanError::InvalidSourceIndex {
                route_index,
                source_index: route.source,
                node_count: nodes.len(),
            });
        }
        if route.destination >= nodes.len() {
            return Err(LatencyPlanError::InvalidDestinationIndex {
                route_index,
                destination_index: route.destination,
                node_count: nodes.len(),
            });
        }
        if matches!(nodes[route.source], LatencyNode::Sink) {
            return Err(LatencyPlanError::SinkUsedAsSource {
                route_index,
                source_index: route.source,
            });
        }

        self.incoming[route.destination].push(route_index);
        if matches!(nodes[route.source], LatencyNode::Processing(_))
            && matches!(nodes[route.destination], LatencyNode::Processing(_))
        {
            self.processing_indegree[route.destination] += 1;
            self.processing_outgoing[route.source].push(route.destination);
        }
        Ok(())
    }

    fn processing_order(&self, nodes: &[LatencyNode]) -> Result<Vec<usize>, LatencyPlanError> {
        let mut indegree = self.processing_indegree.clone();
        let mut ready = VecDeque::with_capacity(self.processing_count);
        for (node_index, node) in nodes.iter().enumerate() {
            if matches!(node, LatencyNode::Processing(_)) && indegree[node_index] == 0 {
                ready.push_back(node_index);
            }
        }

        let mut order = Vec::with_capacity(self.processing_count);
        while let Some(node_index) = ready.pop_front() {
            order.push(node_index);
            for &destination in &self.processing_outgoing[node_index] {
                indegree[destination] -= 1;
                if indegree[destination] == 0 {
                    ready.push_back(destination);
                }
            }
        }
        if order.len() != self.processing_count {
            return Err(LatencyPlanError::ProcessingCycle);
        }
        Ok(order)
    }
}

/// Calculates per-route compensation delays in the same order as `routes`.
///
/// Source arrivals are causal roots and do not depend on their incoming routes. Processing nodes
/// are topologically ordered independently of slice order. A merge with any unknown input is
/// left wholly uncompensated and propagates unknown arrival downstream; `Some(0)` remains known.
///
/// # Errors
/// Returns an error for invalid route indices, a sink used as a source, a processing cycle, or
/// cumulative latency that exceeds `u32`.
pub(crate) fn plan_arrivals(
    nodes: &[LatencyNode],
    routes: &[LatencyRoute],
) -> Result<LatencyPlan, LatencyPlanError> {
    let graph = RouteGraph::new(nodes, routes)?;
    let order = graph.processing_order(nodes)?;
    let mut state = RouteState::new(nodes, routes.len());

    for node_index in order {
        state.process_node(
            node_index,
            nodes[node_index],
            &graph.incoming[node_index],
            routes,
        )?;
    }
    state.align_terminal_merges(nodes, routes, &graph);
    Ok(LatencyPlan {
        delays: state.delays,
        terminal_arrivals: state.terminal_arrivals,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LatencyPlan {
    pub(crate) delays: Vec<u32>,
    pub(crate) terminal_arrivals: Vec<Option<u32>>,
}

#[cfg(test)]
fn route_delays(
    nodes: &[LatencyNode],
    routes: &[LatencyRoute],
) -> Result<Vec<u32>, LatencyPlanError> {
    plan_arrivals(nodes, routes).map(|plan| plan.delays)
}

#[derive(Debug)]
struct RouteState {
    arrivals: Vec<Option<u32>>,
    delays: Vec<u32>,
    terminal_arrivals: Vec<Option<u32>>,
}

impl RouteState {
    fn new(nodes: &[LatencyNode], route_count: usize) -> Self {
        let arrivals = nodes
            .iter()
            .map(|node| match node {
                LatencyNode::Source(arrival) => *arrival,
                LatencyNode::Processing(_) | LatencyNode::Sink => None,
            })
            .collect();
        Self {
            arrivals,
            delays: vec![0; route_count],
            terminal_arrivals: vec![None; nodes.len()],
        }
    }

    fn process_node(
        &mut self,
        node_index: usize,
        node: LatencyNode,
        incoming: &[usize],
        routes: &[LatencyRoute],
    ) -> Result<(), LatencyPlanError> {
        let LatencyNode::Processing(intrinsic_frames) = node else {
            return Ok(());
        };
        let Some(input_arrival) = self.align_merge(incoming, routes) else {
            self.arrivals[node_index] = None;
            return Ok(());
        };
        self.arrivals[node_index] = match intrinsic_frames {
            Some(intrinsic_frames) => Some(
                input_arrival
                    .checked_add(intrinsic_frames)
                    .ok_or(LatencyPlanError::LatencyOverflow { node_index })?,
            ),
            None => None,
        };
        Ok(())
    }

    fn align_merge(&mut self, incoming: &[usize], routes: &[LatencyRoute]) -> Option<u32> {
        let maximum = incoming.iter().try_fold(0_u32, |maximum, route_index| {
            let source = routes[*route_index].source;
            self.arrivals[source].map(|arrival| maximum.max(arrival))
        })?;

        for &route_index in incoming {
            let source = routes[route_index].source;
            if let Some(arrival) = self.arrivals[source] {
                self.delays[route_index] = maximum - arrival;
            }
        }
        Some(maximum)
    }

    fn align_terminal_merges(
        &mut self,
        nodes: &[LatencyNode],
        routes: &[LatencyRoute],
        graph: &RouteGraph,
    ) {
        for (node_index, node) in nodes.iter().enumerate() {
            if matches!(node, LatencyNode::Source(_) | LatencyNode::Sink) {
                self.terminal_arrivals[node_index] = if graph.incoming[node_index].is_empty() {
                    None
                } else {
                    self.align_merge(&graph.incoming[node_index], routes)
                };
            }
        }
    }
}

#[cfg(test)]
#[path = "latency_plan/tests.rs"]
mod tests;
