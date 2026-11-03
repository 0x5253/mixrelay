//! Prepared route history follows cached worker latency, never native plug-in queries.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use mixrelay_audio_graph::{ExecutionPlan, NodeConfig};

use super::{HostedAudioProcessor, Kernel, NodeStep, RenderPlan, route_delay::RouteDelayLine};
use crate::latency_plan::{LatencyNode, LatencyPlanError, LatencyRoute, plan_arrivals};
use crate::runtime_control::ControlValue;

/// Attempts at a consistent read before a window is reported as disturbed.
const ACKNOWLEDGEMENT_READS: usize = 8;

/// Why measured arrivals are unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrivalError {
    /// The request does not match the prepared plan.
    Invalid(String),
    /// Every read overlapped the audio thread acknowledging installed delays; the window
    /// is disturbed, not unknown, so a previously stable value remains valid.
    Contended,
}

const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

/// Retained reader tied to one prepared worker, including before activation.
/// Implementations must only read cached atomics: no allocation, lock, blocking or native calls.
pub trait HostedLatencySource: Send + Sync {
    /// Total transport and intrinsic delay in this worker's prepared sample frames.
    /// `None` means unknown, unhealthy or unrepresentable; `Some(0)` is a known zero.
    fn frames(&self) -> Option<u32>;
}

/// Whether the active graph's derived route alignment is authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompensationState {
    /// No derived history bank has been installed for this processor yet.
    Unprepared,
    /// Every observed route delay is known and the prepared bank is current.
    Current,
    /// At least one source/processor arrival is unknown; affected merges stay unaligned.
    Unknown,
    /// A changed cached latency invalidated the active bank until its replacement arrives.
    Pending,
    /// The complete bank exceeds its aggregate memory budget; no route delay is applied.
    CapacityExceeded,
}

/// Immutable route geometry and retained cached readers, prepared off the audio thread.
pub(super) struct CompensationTopology {
    nodes: Vec<LatencyNode>,
    content_nodes: Vec<LatencyNode>,
    ids: Vec<mixrelay_audio_graph::NodeId>,
    routes: Vec<LatencyRoute>,
    channels: Vec<usize>,
    readers: Vec<(usize, Arc<dyn HostedLatencySource>)>,
    applied: Vec<AtomicU64>,
    applied_sequence: AtomicU64,
}

impl fmt::Debug for CompensationTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompensationTopology")
            .field("nodes", &self.nodes.len())
            .field("content_nodes", &self.content_nodes)
            .field("ids", &self.ids)
            .field("routes", &self.routes.len())
            .field("channels", &self.channels)
            .field("readers", &self.readers.len())
            .field("applied", &self.applied)
            .field("applied_sequence", &self.applied_sequence)
            .finish()
    }
}

impl CompensationTopology {
    /// Binds compiled route order to exact prepared hosted owners, not registry node lookups.
    pub(super) fn new(
        execution: &ExecutionPlan,
        steps: &[NodeStep],
        hosted: &[Box<dyn HostedAudioProcessor>],
    ) -> Self {
        let indices: std::collections::BTreeMap<_, _> = execution
            .nodes()
            .iter()
            .enumerate()
            .map(|(i, node)| (node.id(), i))
            .collect();
        let nodes: Vec<_> = execution
            .nodes()
            .iter()
            .map(|node| match node.config() {
                NodeConfig::AudioDeviceInput(_) | NodeConfig::ApplicationCapture(_) => {
                    LatencyNode::Source(Some(0))
                }
                // Shared/native and graph-produced device signals have independent causal
                // history. Do not invent a zero arrival across that boundary.
                NodeConfig::VirtualEndpoint(_)
                | NodeConfig::VirtualEndpointTransport(_)
                | NodeConfig::DeviceBus { .. } => LatencyNode::Source(None),
                NodeConfig::AudioDeviceOutput(_) | NodeConfig::Recorder { .. } => LatencyNode::Sink,
                NodeConfig::Provider(_) => LatencyNode::Processing(None),
                NodeConfig::Compressor(_)
                | NodeConfig::Effect(_)
                | NodeConfig::Limiter(_)
                | NodeConfig::Mixer(_)
                | NodeConfig::Gain(_) => LatencyNode::Processing(Some(0)),
            })
            .collect();
        let content_nodes = content_nodes(execution, &nodes);
        let routes = execution
            .edges()
            .iter()
            .map(|edge| LatencyRoute {
                source: indices[&edge.source().node_id],
                destination: indices[&edge.destination().node_id],
            })
            .collect();
        let channels = execution
            .edges()
            .iter()
            .map(|edge| {
                execution.nodes()[indices[&edge.source().node_id]]
                    .outputs()
                    .iter()
                    .find(|port| port.id() == &edge.source().port_id)
                    .map_or(0, |port| usize::from(port.channels()))
            })
            .collect();
        let readers: Vec<_> = steps
            .iter()
            .enumerate()
            .flat_map(|(index, step)| {
                step.processing
                    .iter()
                    .filter_map(move |kernel| match kernel {
                        Kernel::Hosted { processor, .. } => hosted[*processor]
                            .latency_source()
                            .map(|reader| (index, reader)),
                        _ => None,
                    })
            })
            .collect();
        Self {
            ids: execution
                .nodes()
                .iter()
                .map(|node| node.id().clone())
                .collect(),
            nodes,
            content_nodes,
            routes,
            channels,
            applied: readers.iter().map(|_| AtomicU64::new(u64::MAX)).collect(),
            applied_sequence: AtomicU64::new(0),
            readers,
        }
    }

    /// Collects one control-side observation per retained hosted owner.
    pub(super) fn observations(&self) -> Vec<Option<u32>> {
        self.readers
            .iter()
            .map(|(_, reader)| reader.frames())
            .collect()
    }

    /// Checks cached atoms only; safe for the audio thread with no retries or allocation.
    pub(super) fn matches(&self, expected: &[Option<u32>]) -> bool {
        expected.len() == self.readers.len()
            && self
                .readers
                .iter()
                .zip(expected)
                .all(|((_, reader), value)| reader.frames() == *value)
    }

    pub(super) fn destination_arrivals(
        &self,
        sources: &[super::ExternalBinding],
        observed: &[Option<u32>],
        sinks: &[super::ExternalBinding],
    ) -> Result<Vec<Option<u32>>, String> {
        if sources.len() != observed.len() {
            return Err("source arrival count mismatch".into());
        }
        let mut nodes = self.nodes.clone();
        for (index, node) in nodes.iter_mut().enumerate() {
            if matches!(node, LatencyNode::Source(_)) {
                let arrival = sources
                    .iter()
                    .position(|binding| binding.node_id() == &self.ids[index])
                    .and_then(|index| observed[index]);
                *node = LatencyNode::Source(arrival);
            }
        }
        for (index, reader) in &self.readers {
            nodes[*index] = LatencyNode::Processing(reader.frames());
        }
        let plan = plan_arrivals(&nodes, &self.routes).map_err(|error| error.to_string())?;
        // Measurements cannot invent compensation histories that audio never installed.
        let (applied_delays, state) = self.delays(&self.observations())?;
        if state == CompensationState::CapacityExceeded || applied_delays != plan.delays {
            return Ok(vec![None; sinks.len()]);
        }
        Ok(sinks
            .iter()
            .map(|binding| {
                self.ids
                    .iter()
                    .position(|id| id == binding.node_id())
                    .and_then(|index| plan.terminal_arrivals[index])
            })
            .collect())
    }

    pub(super) fn destination_source_indexes(&self, plan: &RenderPlan) -> Vec<Vec<usize>> {
        let contributors =
            crate::measured_arrivals::contributors(&self.content_nodes, &self.routes);
        plan.sink_bindings()
            .iter()
            .map(|sink| {
                self.ids
                    .iter()
                    .position(|id| id == sink.node_id())
                    .map_or_else(Vec::new, |index| {
                        contributors[index]
                            .iter()
                            .filter_map(|source| {
                                plan.source_bindings()
                                    .iter()
                                    .position(|binding| binding.node_id() == &self.ids[*source])
                            })
                            .collect()
                    })
            })
            .collect()
    }

    // Prepare destination path membership off the audio thread. Source nodes are
    // causal boundaries: never follow a audio return path's incoming route upstream.
    pub(super) fn binding_routes(&self, id: &mixrelay_audio_graph::NodeId) -> Vec<usize> {
        let mut visited = vec![false; self.nodes.len()];
        if let Some(index) = self.ids.iter().position(|value| value == id) {
            visited[index] = true;
        }
        let mut selected = vec![false; self.routes.len()];
        for _ in 0..self.nodes.len() {
            for (index, route) in self.routes.iter().enumerate() {
                let (from, to) = (route.destination, route.source);
                if visited[from] {
                    selected[index] = true;
                    if matches!(self.content_nodes[to], LatencyNode::Processing(_)) {
                        visited[to] = true;
                    }
                }
            }
        }
        selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect()
    }

    /// Nanosecond roots retain fractional graph frames and never invent new histories.
    /// The audio thread acknowledges installed delays every cycle; a read that keeps
    /// overlapping it reports [`ArrivalError::Contended`] rather than unknown arrivals.
    pub(super) fn measured_destination_arrivals(
        &self,
        plan: &RenderPlan,
        observed: &[Option<f64>],
        threshold_ns: f64,
    ) -> Result<Vec<Option<f64>>, ArrivalError> {
        let sources = plan.source_bindings();
        let rate = plan.format().sample_rate;
        if sources.len() != observed.len() || rate == 0 {
            return Err(ArrivalError::Invalid(
                "invalid measured source arrivals".into(),
            ));
        }
        for _ in 0..ACKNOWLEDGEMENT_READS {
            if let Some(arrivals) = self.read_arrivals(plan, observed, (rate, threshold_ns))? {
                return Ok(arrivals);
            }
            std::hint::spin_loop();
        }
        Err(ArrivalError::Contended)
    }

    /// Holds the acknowledgement open (odd sequence) as the audio thread does mid-write.
    #[cfg(test)]
    pub(super) fn acknowledging(&self, active: bool) {
        if active == (self.applied_sequence.load(Ordering::SeqCst) & 1 == 0) {
            self.applied_sequence.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// One consistent read, or `None` when it overlapped an acknowledgement.
    fn read_arrivals(
        &self,
        plan: &RenderPlan,
        observed: &[Option<f64>],
        (rate, threshold_ns): (u32, f64),
    ) -> Result<Option<Vec<Option<f64>>>, ArrivalError> {
        let sources = plan.source_bindings();
        let sinks = plan.sink_bindings();
        let observations = self.observations();
        let sequence = self.applied_sequence.load(Ordering::SeqCst);
        let acknowledged = self
            .applied
            .iter()
            .zip(&observations)
            .all(|(applied, value)| {
                applied.load(Ordering::SeqCst) == value.map_or(1_u64 << 32, u64::from)
            });
        if sequence & 1 != 0 || self.applied_sequence.load(Ordering::SeqCst) != sequence {
            return Ok(None);
        }
        // A consistent read whose histories are not installed yet is genuinely unknown.
        if !acknowledged {
            return Ok(Some(vec![None; sinks.len()]));
        }
        let (delays, state) = self.delays(&observations).map_err(ArrivalError::Invalid)?;
        if state == CompensationState::CapacityExceeded {
            return Ok(Some(vec![None; sinks.len()]));
        }
        let mut nodes = self.content_nodes.clone();
        for ((index, _), frames) in self.readers.iter().zip(observations.iter().copied()) {
            nodes[*index] = LatencyNode::Processing(frames);
        }
        let roots = self
            .ids
            .iter()
            .map(|id| {
                sources
                    .iter()
                    .position(|binding| binding.node_id() == id)
                    .and_then(|index| observed[index])
            })
            .collect::<Vec<_>>();
        let arrivals = crate::measured_arrivals::arrivals(
            &nodes,
            &self.routes,
            &delays,
            &roots,
            (rate, threshold_ns),
        );
        if !self.matches(&observations) {
            return Ok(None);
        }
        Ok(Some(
            sinks
                .iter()
                .map(|sink| {
                    self.ids
                        .iter()
                        .position(|id| id == sink.node_id())
                        .and_then(|index| arrivals[index])
                })
                .collect(),
        ))
    }

    fn delays(&self, observed: &[Option<u32>]) -> Result<(Vec<u32>, CompensationState), String> {
        if observed.len() != self.readers.len() {
            return Err("latency observation does not match its prepared owners".into());
        }
        let mut nodes = self.nodes.clone();
        for ((index, _), value) in self.readers.iter().zip(observed) {
            nodes[*index] = LatencyNode::Processing(*value);
        }
        let unknown = nodes.iter().any(|node| {
            matches!(
                node,
                LatencyNode::Source(None) | LatencyNode::Processing(None)
            )
        });
        match plan_arrivals(&nodes, &self.routes) {
            Ok(plan) => Ok((
                plan.delays,
                if unknown {
                    CompensationState::Unknown
                } else {
                    CompensationState::Current
                },
            )),
            // Large reported delays must not make the entire plug-in graph unusable.
            Err(LatencyPlanError::LatencyOverflow { .. }) => Ok((
                vec![0; self.routes.len()],
                CompensationState::CapacityExceeded,
            )),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn content_nodes(execution: &ExecutionPlan, nodes: &[LatencyNode]) -> Vec<LatencyNode> {
    // Snapshot reads precede publishes into a prepared-quantum ring. Follow
    // their real feeding routes for content evidence without changing the
    // nominal compensation topology (which cuts causal feedback at buses).
    execution
        .nodes()
        .iter()
        .zip(nodes)
        .map(|(node, latency)| match node.config() {
            NodeConfig::DeviceBus { .. } => {
                LatencyNode::Processing(Some(execution.format().block_frames))
            }
            _ => *latency,
        })
        .collect()
}

/// Mutable history belongs exclusively to the audio owner after block-boundary publication.
pub(crate) struct DelayBank {
    pub(crate) plan: Arc<RenderPlan>,
    observed: Vec<Option<u32>>,
    histories: Vec<Option<RouteDelayLine>>,
    scratch: Vec<f32>,
    control_gates: Vec<[bool; 2]>,
    state: CompensationState,
    valid: bool,
    content_valid: bool,
    content_routes: Vec<bool>,
}

impl DelayBank {
    /// Preallocates the complete history bank on the control thread.
    /// Returns a diagnostic for invalid geometry or allocation failure; excessive memory
    /// demand disables compensation as a whole without refusing the plug-in graph.
    pub(crate) fn prepare(
        plan: Arc<RenderPlan>,
        observed: Vec<Option<u32>>,
    ) -> Result<Self, String> {
        let topology = &plan.compensation;
        let (delays, state) = topology.delays(&observed)?;
        let storage = DelayStorage::prepare(&delays, &topology.channels, plan.format.block_frames)?;
        let control_gates = plan
            .controls
            .values()
            .iter()
            .map(|value| [value.control.enabled, value.control.muted])
            .collect();
        Ok(Self {
            plan,
            observed,
            histories: storage.histories,
            scratch: storage.scratch,
            control_gates,
            state: if storage.over_budget {
                CompensationState::CapacityExceeded
            } else {
                state
            },
            valid: true,
            content_valid: false,
            content_routes: vec![false; delays.len()],
        })
    }

    /// Distinguishes a stale bank from a prepared unknown/over-budget one.
    pub(crate) fn state(&self) -> CompensationState {
        if self.valid {
            self.state
        } else {
            CompensationState::Pending
        }
    }

    pub(crate) fn acknowledge_content_latency(&self) -> bool {
        let topology = &self.plan.compensation;
        topology.applied_sequence.fetch_add(1, Ordering::SeqCst);
        for (applied, value) in topology.applied.iter().zip(&self.observed) {
            applied.store(value.map_or(1_u64 << 32, u64::from), Ordering::SeqCst);
        }
        topology.applied_sequence.fetch_add(1, Ordering::SeqCst);
        self.valid
            && self.content_valid
            && matches!(
                self.state,
                CompensationState::Current | CompensationState::Unknown
            )
            && self.histories.iter().flatten().all(RouteDelayLine::primed)
    }

    pub(super) fn content_ready_for_routes(&self, routes: &[usize]) -> bool {
        self.valid
            && matches!(
                self.state,
                CompensationState::Current | CompensationState::Unknown
            )
            && routes.iter().all(|&index| self.content_routes[index])
    }

    /// Equal derived observations on the same immutable topology need no history replacement.
    pub(super) fn equivalent(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.plan, &other.plan) && self.observed == other.observed
    }

    /// Invalidates history before rendering when cached worker latency no longer matches.
    pub(crate) fn validate(&mut self) {
        let valid = self.plan.compensation.matches(&self.observed);
        if !valid && self.valid {
            self.reset();
        }
        self.valid = valid;
        self.content_valid = valid;
        for (ready, history) in self.content_routes.iter_mut().zip(&self.histories) {
            *ready = valid && history.as_ref().is_none_or(RouteDelayLine::primed);
        }
    }

    /// Invalidates every ring cursor without touching its backing PCM allocation.
    pub(crate) fn reset(&mut self) {
        self.content_valid = false;
        self.content_routes.fill(false);
        for history in self.histories.iter_mut().flatten() {
            history.reset();
        }
    }

    /// Invalidates only route indices prepared from this bank's immutable topology.
    pub(super) fn reset_routes(&mut self, routes: &[usize]) {
        for &index in routes {
            self.content_routes[index] = false;
            if let Some(history) = &mut self.histories[index] {
                history.reset();
            }
        }
    }

    /// Gate transitions cannot replay pre-OFF/pre-mute history; gain changes retain it.
    pub(crate) fn observe_controls(&mut self, controls: &[ControlValue]) {
        for (route, history) in self
            .plan
            .compensation
            .routes
            .iter()
            .zip(&mut self.histories)
        {
            let changed = [route.source, route.destination].iter().any(|&i| {
                self.control_gates[i] != [controls[i].control.enabled, controls[i].control.muted]
            });
            if changed && let Some(history) = history {
                history.reset();
            }
        }
        for (gate, value) in self.control_gates.iter_mut().zip(controls) {
            *gate = [value.control.enabled, value.control.muted];
        }
    }

    /// Borrows a delayed full-width source until the next route; absent histories stay direct.
    pub(super) fn delayed<'a>(
        &'a mut self,
        index: usize,
        source: &[f32],
        frames: usize,
    ) -> Option<&'a [f32]> {
        if !self.valid {
            return None;
        }
        let history = self.histories.get_mut(index)?.as_mut()?;
        self.content_routes[index] &= history.primed();
        self.content_valid &= history.primed();
        let output = self.scratch.get_mut(..source.len())?;
        history.advance(source, output, frames).then_some(output)
    }
}

struct DelayStorage {
    histories: Vec<Option<RouteDelayLine>>,
    scratch: Vec<f32>,
    over_budget: bool,
}

impl DelayStorage {
    fn prepare(delays: &[u32], channels: &[usize], block_frames: u32) -> Result<Self, String> {
        let scratch_samples = channels
            .iter()
            .zip(delays)
            .filter_map(|(&channels, &delay)| (delay != 0).then_some(channels))
            .max()
            .unwrap_or(0)
            .checked_mul(block_frames as usize)
            .ok_or("delay scratch overflow")?;
        let samples =
            delays
                .iter()
                .zip(channels)
                .try_fold(scratch_samples, |total, (&delay, &channels)| {
                    (delay as usize)
                        .checked_mul(channels)
                        .and_then(|n| total.checked_add(n))
                });
        let over_budget = samples.is_none_or(|n| n > MAX_HISTORY_BYTES / size_of::<f32>());
        let mut histories = Vec::with_capacity(delays.len());
        for (&delay, &channels) in delays.iter().zip(channels) {
            histories.push(if delay == 0 || over_budget {
                None
            } else {
                Some(RouteDelayLine::new(channels, delay as usize).map_err(|e| e.to_string())?)
            });
        }
        let mut scratch = Vec::new();
        let scratch_samples = if over_budget || delays.iter().all(|&n| n == 0) {
            0
        } else {
            scratch_samples
        };
        scratch
            .try_reserve_exact(scratch_samples)
            .map_err(|e| e.to_string())?;
        scratch.resize(scratch_samples, 0.0);
        let allocated = histories
            .iter()
            .flatten()
            .try_fold(scratch.capacity(), |n, history| {
                n.checked_add(history.history_samples())
            });
        if allocated.is_none_or(|samples| samples > MAX_HISTORY_BYTES / size_of::<f32>()) {
            return Err("allocator exceeded the route history budget".into());
        }
        Ok(Self {
            histories,
            scratch,
            over_budget,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_history_and_scratch_share_one_budget() {
        let exact = DelayStorage::prepare(&[65_535], &[64], 1).unwrap();
        assert!(!exact.over_budget);
        assert_eq!(
            exact.histories[0].as_ref().unwrap().history_samples() + exact.scratch.capacity(),
            MAX_HISTORY_BYTES / 4
        );
        let excess = DelayStorage::prepare(&[65_536], &[64], 1).unwrap();
        assert!(excess.over_budget);
        assert!(excess.histories.iter().all(Option::is_none));
        assert!(excess.scratch.is_empty());
    }

    #[test]
    fn enormous_delay_does_not_allocate_or_disable_hosted_processing() {
        let storage = DelayStorage::prepare(&[u32::MAX], &[64], 4096).unwrap();
        assert!(storage.over_budget);
        assert!(storage.histories[0].is_none());
        assert_eq!(storage.scratch.capacity(), 0);
    }

    #[test]
    fn direct_routes_need_no_history_or_scratch() {
        let storage = DelayStorage::prepare(&[0, 0], &[32, 64], 4096).unwrap();
        assert!(!storage.over_budget);
        assert!(storage.histories.iter().all(Option::is_none));
        assert_eq!(storage.scratch.capacity(), 0);
    }
}
