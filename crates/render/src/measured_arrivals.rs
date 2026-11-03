//! Evaluate measured roots against the histories the nominal plan actually installs.
use crate::latency_plan::{LatencyNode, LatencyRoute};

/// Causal source node indexes feeding each terminal binding (including audio return path nodes).
pub(crate) fn contributors(nodes: &[LatencyNode], routes: &[LatencyRoute]) -> Vec<Vec<usize>> {
    let mut values: Vec<Vec<usize>> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            if matches!(node, LatencyNode::Source(_)) {
                vec![index]
            } else {
                Vec::new()
            }
        })
        .collect();
    for _ in 0..nodes.len() {
        let mut changed = false;
        for route in routes {
            if matches!(nodes[route.destination], LatencyNode::Processing(_)) {
                let before = values[route.destination].len();
                let source = values[route.source].clone();
                values[route.destination].extend(source);
                values[route.destination].sort_unstable();
                values[route.destination].dedup();
                changed |= before != values[route.destination].len();
            }
        }
        if !changed {
            break;
        }
    }
    nodes
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let mut sources: Vec<_> = routes
                .iter()
                .filter(|route| route.destination == index)
                .flat_map(|route| values[route.source].iter().copied())
                .collect();
            sources.sort_unstable();
            sources.dedup();
            sources
        })
        .collect()
}

pub(crate) fn arrivals(
    nodes: &[LatencyNode],
    routes: &[LatencyRoute],
    delays: &[u32],
    roots: &[Option<f64>],
    timing: (u32, f64),
) -> Vec<Option<f64>> {
    let (rate, threshold_ns) = timing;
    let mut values: Vec<Option<(f64, f64)>> =
        roots.iter().map(|root| root.map(|v| (v, v))).collect();
    let mut visited: Vec<_> = nodes
        .iter()
        .map(|node| matches!(node, LatencyNode::Source(_)))
        .collect();
    for _ in 0..nodes.len() {
        for (index, node) in nodes.iter().enumerate() {
            if visited[index] || !matches!(node, LatencyNode::Processing(_)) {
                continue;
            }
            if routes
                .iter()
                .filter(|route| route.destination == index)
                .any(|route| !visited[route.source])
            {
                continue;
            }
            visited[index] = true;
            if let LatencyNode::Processing(Some(frames)) = node {
                let delay = f64::from(*frames) * 1e9 / f64::from(rate);
                values[index] = merge(index, routes, delays, &values, rate)
                    .map(|(min, max)| (min + delay, max + delay));
            }
        }
    }
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            if !matches!(node, LatencyNode::Source(_) | LatencyNode::Sink)
                || !routes.iter().any(|route| route.destination == index)
            {
                return None;
            }
            let (min, max) = merge(index, routes, delays, &values, rate)?;
            (min.is_finite() && min >= 0.0 && max.is_finite() && max - min <= threshold_ns)
                .then_some(max)
        })
        .collect()
}

fn merge(
    index: usize,
    routes: &[LatencyRoute],
    delays: &[u32],
    values: &[Option<(f64, f64)>],
    rate: u32,
) -> Option<(f64, f64)> {
    let mut range: Option<(f64, f64)> = None;
    for (route, delay) in routes
        .iter()
        .zip(delays)
        .filter(|(route, _)| route.destination == index)
    {
        let (min, max) = values[route.source]?;
        let delay = f64::from(*delay) * 1e9 / f64::from(rate);
        range = Some(range.map_or((min + delay, max + delay), |(low, high)| {
            (low.min(min + delay), high.max(max + delay))
        }));
    }
    Some(range.unwrap_or((0.0, 0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_compensation_preserves_measured_roots_and_requires_total_agreement() {
        let nodes = [
            LatencyNode::Source(Some(0)),
            LatencyNode::Processing(Some(48)),
            LatencyNode::Sink,
            LatencyNode::Source(None),
            LatencyNode::Processing(None),
        ];
        let routes = [
            LatencyRoute {
                source: 0,
                destination: 1,
            },
            LatencyRoute {
                source: 0,
                destination: 2,
            },
            LatencyRoute {
                source: 1,
                destination: 2,
            },
            LatencyRoute {
                source: 3,
                destination: 4,
            },
        ];
        let roots = [Some(0.25), None, None, None, None];
        assert_eq!(contributors(&nodes, &routes)[2], vec![0]);
        let result = arrivals(&nodes, &routes, &[0, 48, 0, 0], &roots, (48_000, 1e6));
        assert_eq!(result[2], Some(1_000_000.25));
        assert_eq!(result[3], None);
        assert_eq!(
            arrivals(&nodes, &routes, &[0, 0, 0, 0], &roots, (48_000, 999_999.0))[2],
            None
        );
        let mut unknown = roots;
        unknown[0] = None;
        assert_eq!(
            arrivals(&nodes, &routes, &[0, 48, 0, 0], &unknown, (48_000, 1e6))[2],
            None
        );
        unknown[0] = Some(-1.0);
        assert_eq!(
            arrivals(&nodes, &routes, &[0, 0, 0, 0], &unknown, (48_000, 1e6))[2],
            None
        );
    }

    #[test]
    fn multiple_sources_must_agree_without_inventing_source_compensation() {
        let nodes = [
            LatencyNode::Source(Some(0)),
            LatencyNode::Source(Some(0)),
            LatencyNode::Sink,
        ];
        let routes = [
            LatencyRoute {
                source: 0,
                destination: 2,
            },
            LatencyRoute {
                source: 1,
                destination: 2,
            },
        ];
        assert_eq!(contributors(&nodes, &routes)[2], vec![0, 1]);
        for (second, expected) in [
            (Some(2e6), Some(2e6)),
            (Some(2_000_000.25), None),
            (None, None),
        ] {
            assert_eq!(
                arrivals(
                    &nodes,
                    &routes,
                    &[0, 0],
                    &[Some(1e6), second, None],
                    (48_000, 1e6)
                )[2],
                expected
            );
        }
    }

    #[test]
    fn processing_arrivals_and_contributors_follow_topology_when_nodes_are_shuffled() {
        let nodes = [
            LatencyNode::Processing(Some(24)),
            LatencyNode::Processing(Some(48)),
            LatencyNode::Sink,
            LatencyNode::Source(Some(0)),
        ];
        let routes = [
            LatencyRoute {
                source: 3,
                destination: 1,
            },
            LatencyRoute {
                source: 1,
                destination: 0,
            },
            LatencyRoute {
                source: 0,
                destination: 2,
            },
        ];
        assert_eq!(contributors(&nodes, &routes)[2], vec![3]);
        assert_eq!(
            arrivals(
                &nodes,
                &routes,
                &[0, 0, 0],
                &[None, None, None, Some(0.25)],
                (48_000, 1e6)
            )[2],
            Some(1_500_000.25)
        );
    }
}
