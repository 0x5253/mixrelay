//! Prepared render topology and composition of the block processor's owned resources.
use std::{collections::BTreeMap, sync::Arc};

use mixrelay_audio_graph::{
    AudioEndpoint, CompiledNode, ExecutionPlan, NodeConfig, NodeId, PortDirection, PortId,
    RenderFormat,
};
use mixrelay_audio_io::VirtualEndpointId;
use mixrelay_audio_io::pcm::sanitize_samples;
use mixrelay_audio_io::process::SampleFaults;
use thiserror::Error;

use crate::runtime_control::{ControlValue, RuntimeControlBank};
mod buffers;
mod compensation;
mod cycle;
pub use compensation::ArrivalError;
mod effects;
mod external_io;
pub use external_io::{CycleAdmission, ExternalControlState, ExternalIo};
mod history;
mod hosted;
use hosted::HostedRack;
pub use hosted::{
    HostedAudioProcessor, HostedProcessorFactory, HostedProcessorIdentity,
    HostedProcessorPrepareRequest,
};
mod input_timeline;
mod kernels;
mod ownership;
mod prepare;
mod recording;
pub use recording::{RecordingSink, RecordingSinkError, RecordingSinks, recording_session_key};
mod route_delay;
mod routing;
mod schedule;
mod source_routes;
use buffers::port_pair;
pub(crate) use compensation::DelayBank;
pub use compensation::{CompensationState, HostedLatencySource};
use kernels::{KernelResources, apply_kernel};
use routing::{RouteOp, apply_routes};
use schedule::RenderSchedule;

type HostedProcessors = Box<[Box<dyn HostedAudioProcessor>]>;
type PreparedRenderPlan = (RenderPlan, HostedProcessors);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalBinding {
    node_id: NodeId,
    kind: mixrelay_audio_graph::NodeKind,
    port_id: PortId,
    channels: u16,
    virtual_endpoint_id: Option<VirtualEndpointId>,
    routed_source: bool,
}

#[derive(Debug, Clone)]
pub struct MeterBinding {
    node_id: NodeId,
    port_id: PortId,
    channels: u16,
    slot: PortSlot,
    dynamics: Option<DynamicsMeterBinding>,
}

#[derive(Debug, Clone, Copy)]
struct DynamicsMeterBinding {
    input: PortSlot,
    state_index: usize,
    threshold_db: f32,
}

impl MeterBinding {
    #[must_use]
    pub const fn has_dynamics(&self) -> bool {
        self.dynamics.is_some()
    }
    #[must_use]
    pub const fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    #[must_use]
    pub const fn port_id(&self) -> &PortId {
        &self.port_id
    }

    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }
}

impl ExternalBinding {
    /// Whether this source reaches an external output or recorder. Sources
    /// without such a path are drained and metered independently of routed inputs.
    #[must_use]
    pub const fn is_routed_source(&self) -> bool {
        self.routed_source
    }

    #[must_use]
    pub const fn kind(&self) -> mixrelay_audio_graph::NodeKind {
        self.kind
    }

    /// Canonical device identity from the node configuration, independent of
    /// graph-local node names. Hardware and application bindings return `None`.
    #[must_use]
    pub const fn virtual_endpoint_id(&self) -> Option<&VirtualEndpointId> {
        self.virtual_endpoint_id.as_ref()
    }

    #[must_use]
    pub const fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    #[must_use]
    pub const fn port_id(&self) -> &PortId {
        &self.port_id
    }

    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }
}

#[derive(Debug)]
pub struct RenderPlan {
    revision: u64,
    format: RenderFormat,
    steps: Box<[NodeStep]>,
    schedule: RenderSchedule,
    sink_routes: Box<[RouteOp]>,
    source_bindings: Box<[ExternalBinding]>,
    sink_bindings: Box<[ExternalBinding]>,
    meter_bindings: Box<[MeterBinding]>,
    recorder_bindings: Box<[MeterBinding]>,
    arena_samples: usize,
    /// Every port slot, so a block shorter than the prepared quantum clears only
    /// the frames it renders instead of the whole arena.
    arena_slots: Box<[PortSlot]>,
    dynamics_state_count: usize,
    dynamics_bindings: Box<[DynamicsBinding]>,
    effects: Box<[effects::Binding]>,
    device_bus_samples: usize,
    device_bus_bindings: Box<[DeviceBusBinding]>,
    history: Box<[history::Binding]>,
    source_history: Box<[history::Targets]>,
    hosted_input_scratch_samples: usize,
    hosted_scratch_samples: usize,
    controls: Arc<RuntimeControlBank>,
    source_controls: Box<[usize]>,
    sink_controls: Box<[usize]>,
    recorder_controls: Box<[usize]>,
    compensation: compensation::CompensationTopology,
}

impl RenderPlan {
    /// Lowers a validated graph plan into numeric buffer offsets and render operations.
    ///
    /// # Errors
    ///
    /// Returns [`RenderPlanError`] when a custom resolver produced ports that do not match
    /// the node contract or the render arena size overflows.
    pub fn prepare(execution: &ExecutionPlan) -> Result<Self, RenderPlanError> {
        let (plan, processors) = prepare::plan(execution, None)?;
        debug_assert!(processors.is_empty());
        Ok(plan)
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Stable control handles for this prepared topology, shared with its audio processor.
    #[must_use]
    pub const fn controls(&self) -> &Arc<RuntimeControlBank> {
        &self.controls
    }

    #[must_use]
    pub const fn format(&self) -> RenderFormat {
        self.format
    }

    #[must_use]
    pub fn source_bindings(&self) -> &[ExternalBinding] {
        &self.source_bindings
    }

    #[must_use]
    pub fn sink_bindings(&self) -> &[ExternalBinding] {
        &self.sink_bindings
    }

    #[must_use]
    pub fn meter_bindings(&self) -> &[MeterBinding] {
        &self.meter_bindings
    }

    #[must_use]
    pub const fn arena_samples(&self) -> usize {
        self.arena_samples
    }

    /// Computes terminal arrivals including processing and inserted compensation on control.
    /// Supply measured causal source delays in source-binding order, in graph-rate frames.
    /// Unresolved sources propagate `None` only to destinations they feed.
    /// # Errors
    /// Returns invalid source geometry or an unrepresentable accumulated delay.
    pub fn destination_arrivals(
        &self,
        sources: &[Option<u32>],
    ) -> Result<Vec<Option<u32>>, String> {
        self.compensation
            .destination_arrivals(&self.source_bindings, sources, &self.sink_bindings)
    }

    /// Samples cached worker delays on the control thread, retaining prepared owner order.
    pub(crate) fn latency_observations(&self) -> Vec<Option<u32>> {
        self.compensation.observations()
    }

    /// Control-side causal source binding indexes for each destination binding.
    #[must_use]
    pub fn destination_source_indexes(&self) -> Vec<Vec<usize>> {
        self.compensation.destination_source_indexes(self)
    }

    /// Control-side content arrivals in nanoseconds under existing route compensation.
    /// Unlike integer source-frame observations, fractional measured roots are retained.
    /// Contributors must agree within the supplied nanosecond threshold.
    ///
    /// # Errors
    /// Rejects invalid source counts, rate or an invalid compensation plan, and reports
    /// reads that kept overlapping the audio thread's acknowledgement as contended.
    pub fn measured_destination_arrivals(
        &self,
        sources_ns: &[Option<f64>],
        threshold_ns: f64,
    ) -> Result<Vec<Option<f64>>, ArrivalError> {
        self.compensation
            .measured_destination_arrivals(self, sources_ns, threshold_ns)
    }

    /// Compares cached worker delays without allocating or querying native plug-ins.
    pub(crate) fn latency_matches(&self, expected: &[Option<u32>]) -> bool {
        self.compensation.matches(expected)
    }
}

pub struct BlockProcessor {
    recorders: Box<[Box<dyn RecordingSink>]>,
    cycle_tracker: mixrelay_audio_io::process::CycleTracker,
    plan: Arc<RenderPlan>,
    arena: Box<[f32]>,
    source_buffers: ExternalBuffers,
    sink_buffers: ExternalBuffers,
    source_latency_faults: Box<[bool]>,
    source_timelines: Box<[input_timeline::State]>,
    sink_latency_faults: Box<[bool]>,
    sink_latency_routes: Vec<Vec<usize>>,
    meters: Option<Arc<crate::PeakMeterBank>>,
    dynamics_envelopes: Box<[f32]>,
    dynamics_minimum_gains: Box<[f32]>,
    dynamics_wet: Box<[f32]>,
    effect_states: Box<[effects::State]>,
    device_bus_state: Box<[f32]>,
    device_bus_cursor: usize,
    hosted: HostedRack,
    controls: Box<[ControlValue]>,
    control_scratch: Box<[ControlValue]>,
    sampled_control_epoch: Option<u64>,
    delay_bank: Option<Box<DelayBank>>,
}

type ExternalBuffers = Box<[Box<[f32]>]>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RenderPlanError {
    #[error("render buffer preparation failed: {0}")]
    BufferPreparation(String),
    #[error("built-in effect preparation failed: {0}")]
    BuiltinPreparation(String),
    /// Derived route alignment could not be prepared before publication.
    #[error("route delay preparation failed: {0}")]
    CompensationPreparation(String),
    #[error("render buffer size overflowed")]
    BufferSizeOverflow,
    #[error(
        "virtual device source at node {node_id} must be lowered by graph composition before rendering"
    )]
    UnloweredVirtualEndpoint { node_id: NodeId },
    #[error("node {node_id} does not expose required {port_id} port")]
    MissingPort { node_id: NodeId, port_id: PortId },
    #[error("duplicate compiled port {node_id}:{port_id}")]
    DuplicatePort { node_id: NodeId, port_id: PortId },
    #[error("provider node {node_id} requires a registered render kernel")]
    UnsupportedProvider { node_id: NodeId },
    /// A hosted provider direction has zero or too many buses.
    #[error("provider node {node_id} has {found} {direction} buses; expected 1..={maximum}")]
    InvalidProviderBusCount {
        /// Provider whose layout failed validation.
        node_id: NodeId,
        /// Input or output direction.
        direction: &'static str,
        /// Maximum accepted bus count.
        maximum: usize,
        /// Actual bus count.
        found: usize,
    },
    /// A hosted provider bus has zero or too many channels.
    #[error("provider node {node_id} has invalid {direction} bus channel count {channels}")]
    InvalidProviderBusChannels {
        /// Provider whose layout failed validation.
        node_id: NodeId,
        /// Input or output direction.
        direction: &'static str,
        /// Invalid bus channel count.
        channels: u16,
    },
    /// A hosted provider direction exceeds the aggregate channel bound.
    #[error(
        "provider node {node_id} has {channels} total {direction} channels; maximum is {maximum}"
    )]
    InvalidProviderTotalChannels {
        /// Provider whose layout failed validation.
        node_id: NodeId,
        /// Input or output direction.
        direction: &'static str,
        /// Maximum accepted aggregate channels.
        maximum: u16,
        /// Actual aggregate channel count.
        channels: u16,
    },
    /// A multibus provider lacks a selected layout contract.
    #[error("provider node {node_id} has no selected multibus layout")]
    MissingProviderLayout {
        /// Provider whose layout is missing.
        node_id: NodeId,
    },
    /// The selected provider layout does not match the compiled port order.
    #[error("provider node {node_id} layout does not match its compiled ports")]
    ProviderLayoutMismatch {
        /// Provider whose compiled geometry differs from its saved layout.
        node_id: NodeId,
    },
    /// The saved multibus provider layout failed graph contract validation.
    #[error("provider node {node_id} has invalid selected layout: {reason}")]
    InvalidProviderLayout {
        /// Provider whose saved layout was invalid.
        node_id: NodeId,
        /// Typed layout diagnostic.
        reason: String,
    },
    #[error("provider node {node_id} could not be prepared: {reason}")]
    ProviderPreparation { node_id: NodeId, reason: String },
    /// An external or recorder binding names a node absent from the compiled controls.
    #[error("binding at node {node_id} has no compiled runtime control")]
    UncompiledBinding { node_id: NodeId },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProcessBlockError {
    #[error("native adapter reported more frames than requested")]
    InvalidTransfer,
    #[error("invalid audio process cycle: {0}")]
    InvalidCycle(mixrelay_audio_io::process::CycleError),
    #[error("frame count {found} exceeds prepared maximum {maximum}")]
    FrameCountExceeded { maximum: u32, found: u32 },
    #[error("expected {expected} source buffers, found {found}")]
    SourceCountMismatch { expected: usize, found: usize },
    #[error("expected {expected} sink buffers, found {found}")]
    SinkCountMismatch { expected: usize, found: usize },
    #[error("source buffer {index} requires {required} samples, found {found}")]
    SourceBufferTooShort {
        index: usize,
        required: usize,
        found: usize,
    },
    #[error("sink buffer {index} requires {required} samples, found {found}")]
    SinkBufferTooShort {
        index: usize,
        required: usize,
        found: usize,
    },
}

#[derive(Debug, Clone, Copy)]
struct PortSlot {
    offset: usize,
    channels: u16,
}

#[derive(Debug, Clone, Copy)]
struct DryBusMap {
    source_channel_offset: usize,
    destination_channel_offset: usize,
    channels: u16,
}

#[derive(Debug)]
struct DynamicsBinding {
    node_id: NodeId,
    config: NodeConfig,
    state_index: usize,
}

#[derive(Debug)]
struct DeviceBusBinding {
    node_id: NodeId,
    samples: std::ops::Range<usize>,
    control_index: usize,
    input_path: history::InputPath,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PortKey {
    endpoint: AudioEndpoint,
    direction: PortDirection,
}

#[derive(Debug)]
struct NodeStep {
    incoming_routes: Box<[RouteOp]>,
    sources: Box<[Kernel]>,
    processing: Box<[Kernel]>,
    sinks: Box<[Kernel]>,
    inputs: Box<[PortSlot]>,
    outputs: Box<[PortSlot]>,
}

#[derive(Clone, Copy)]
enum RenderPhase {
    Sources,
    Processing,
    Sinks,
}

impl NodeStep {
    fn kernels(&self, phase: RenderPhase) -> &[Kernel] {
        match phase {
            RenderPhase::Sources => &self.sources,
            RenderPhase::Processing => &self.processing,
            RenderPhase::Sinks => &self.sinks,
        }
    }
}

fn silence_slots(arena: &mut [f32], slots: &[PortSlot], frames: usize) {
    for slot in slots {
        arena[slot.offset..slot.offset + sample_count(frames, slot.channels)].fill(0.0);
    }
}

fn sanitize_slots(arena: &mut [f32], slots: &[PortSlot], frames: usize) -> u64 {
    slots.iter().fold(0, |count, slot| {
        count
            + sanitize_samples(
                &mut arena[slot.offset..slot.offset + sample_count(frames, slot.channels)],
            )
    })
}

#[derive(Debug, Default)]
struct LoweredRoutes {
    incoming: BTreeMap<NodeId, Vec<RouteOp>>,
    sinks: Vec<RouteOp>,
}

#[derive(Debug, Default)]
struct LoweredKernels {
    sources: Vec<Kernel>,
    processing: Vec<Kernel>,
    sinks: Vec<Kernel>,
}

#[derive(Debug)]
enum Kernel {
    Effect {
        input: PortSlot,
        output: PortSlot,
        sidechain: Option<PortSlot>,
        state: usize,
    },
    DeviceBus(DeviceBusKernel),
    Source {
        binding: usize,
        output: PortSlot,
    },
    Sink {
        binding: usize,
        input: PortSlot,
    },
    Gain {
        input: PortSlot,
        output: PortSlot,
    },
    Compressor {
        input: PortSlot,
        output: PortSlot,
        state_index: usize,
        threshold_linear: f32,
        exponent: f32,
        attack_coefficient: f32,
        release_coefficient: f32,
        makeup_gain: f32,
        bypass: bool,
        bypass_coefficient: f32,
    },
    Limiter {
        input: PortSlot,
        output: PortSlot,
        state_index: usize,
        ceiling_linear: f32,
        release_coefficient: f32,
        bypass: bool,
        bypass_coefficient: f32,
    },
    Mixer {
        inputs: Box<[PortSlot]>,
        output: PortSlot,
    },
    Hosted {
        io: HostedIo,
        processor: usize,
    },
}

#[derive(Debug)]
enum HostedIo {
    Single {
        input: PortSlot,
        output: PortSlot,
    },
    Multiple {
        inputs: Box<[PortSlot]>,
        outputs: Box<[PortSlot]>,
        input_channels: u16,
        output_channels: u16,
        dry_maps: Box<[DryBusMap]>,
    },
}

#[derive(Debug)]
enum DeviceBusKernel {
    Read {
        snapshot: PortSlot,
        output: PortSlot,
        capacity: usize,
    },
    Publish {
        snapshot: PortSlot,
        input: PortSlot,
        capacity: usize,
    },
}

impl DeviceBusKernel {
    fn apply(&self, arena: &mut [f32], state: &mut [f32], frames: usize, cursor: usize) {
        match self {
            Self::Read {
                snapshot,
                output,
                capacity,
            } => {
                let samples = sample_count(frames, output.channels);
                let offset = sample_count(cursor, output.channels);
                let head = samples.min(capacity - offset);
                let source = &state[snapshot.offset..snapshot.offset + capacity];
                let destination = &mut arena[output.offset..output.offset + samples];
                destination[..head].copy_from_slice(&source[offset..offset + head]);
                destination[head..].copy_from_slice(&source[..samples - head]);
            }
            Self::Publish {
                snapshot,
                input,
                capacity,
            } => {
                let samples = sample_count(frames, input.channels);
                let offset = sample_count(cursor, input.channels);
                let head = samples.min(capacity - offset);
                let destination = &mut state[snapshot.offset..snapshot.offset + capacity];
                let source = &arena[input.offset..input.offset + samples];
                // Only replace the frames consumed in the source phase. A short
                // quantum leaves the unread tail owned by the following cycle.
                destination[offset..offset + head].copy_from_slice(&source[..head]);
                destination[..samples - head].copy_from_slice(&source[head..]);
            }
        }
    }
}

fn allocate_slots(
    nodes: &[CompiledNode],
    block_frames: usize,
) -> Result<(BTreeMap<PortKey, PortSlot>, usize), RenderPlanError> {
    let mut slots = BTreeMap::new();
    let mut next_offset = 0_usize;
    for node in nodes {
        for port in node.inputs().iter().chain(node.outputs()) {
            let key = PortKey {
                endpoint: AudioEndpoint {
                    node_id: node.id().clone(),
                    port_id: port.id().clone(),
                },
                direction: port.direction(),
            };
            let sample_count = block_frames
                .checked_mul(usize::from(port.channels()))
                .ok_or(RenderPlanError::BufferSizeOverflow)?;
            let slot = PortSlot {
                offset: next_offset,
                channels: port.channels(),
            };
            next_offset = next_offset
                .checked_add(sample_count)
                .ok_or(RenderPlanError::BufferSizeOverflow)?;
            if slots.insert(key.clone(), slot).is_some() {
                return Err(RenderPlanError::DuplicatePort {
                    node_id: key.endpoint.node_id,
                    port_id: key.endpoint.port_id,
                });
            }
        }
    }
    Ok((slots, next_offset))
}

fn validate_source_lengths(
    frames: usize,
    bindings: &[ExternalBinding],
    sources: &[&[f32]],
) -> Result<(), ProcessBlockError> {
    for (index, (binding, samples)) in bindings.iter().zip(sources).enumerate() {
        let required = sample_count(frames, binding.channels);
        if samples.len() < required {
            return Err(ProcessBlockError::SourceBufferTooShort {
                index,
                required,
                found: samples.len(),
            });
        }
    }
    Ok(())
}

fn validate_sink_lengths(
    frames: usize,
    bindings: &[ExternalBinding],
    sinks: &[&mut [f32]],
) -> Result<(), ProcessBlockError> {
    for (index, (binding, samples)) in bindings.iter().zip(sinks).enumerate() {
        let required = sample_count(frames, binding.channels);
        if samples.len() < required {
            return Err(ProcessBlockError::SinkBufferTooShort {
                index,
                required,
                found: samples.len(),
            });
        }
    }
    Ok(())
}

const fn sample_count(frames: usize, channels: u16) -> usize {
    frames * channels as usize
}

#[cfg(test)]
mod tests;
