//! Composes every running graph into one DSP plan referencing Driver-owned devices.
//!
//! Each render client sends only the sum of graph contributions. The Driver sums
//! OS playback natively. Sources read the last completed Driver capture snapshot.
//! This fixed one-quantum device boundary is independent of routing topology.
//! Graph client lifetime follows references; Driver lifetime is independent of graphs.
//! All preparation runs on the control plane.

use crate::{
    AUDIO_PORT_ID, AudioEndpoint, CompileContext, CompileError, EdgeId, EdgeSpec, ExecutionPlan,
    FROM_OS_PORT_ID, GainConfig, GraphCompiler, GraphDocument, MixerConfig, NodeConfig, NodeId,
    NodeSpec, PortId, TO_OS_PORT_ID, VirtualEndpointId, VirtualEndpointSpec,
};
use std::collections::{BTreeMap, BTreeSet};

mod authored;
mod lowering;
pub use authored::{AuthoredGraph, ResolvedNodeIdentities, lower_document};

const MAX_IDENTITIES: usize = 256;

/// Validated canonical specifications shared by every graph. The compiler emits native
/// clients only while at least one supplied graph references them. The Driver remains
/// authoritative, including devices with no current graph references.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRegistry {
    devices: BTreeMap<VirtualEndpointId, VirtualEndpointSpec>,
    sources: BTreeMap<String, NodeConfig>,
    outputs: BTreeMap<String, crate::AudioDeviceOutputConfig>,
}

impl DeviceRegistry {
    /// Builds a validated registry without merging two entries that happen to share a name.
    /// # Errors
    /// Rejects duplicate stable IDs, invalid specs, and an unbounded registry.
    pub fn new(devices: Vec<VirtualEndpointSpec>) -> Result<Self, CompositionError> {
        if devices.len() > MAX_IDENTITIES {
            return Err(invalid("too many shared devices"));
        }
        let mut registered = BTreeMap::new();
        for device in devices {
            validate_id(device.id.as_str())?;
            device
                .validate_contract(64, 64)
                .map_err(|error| invalid(error.to_string()))?;
            if registered.insert(device.id.clone(), device).is_some() {
                return Err(invalid("duplicate shared device identity"));
            }
        }
        Ok(Self {
            devices: registered,
            sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
        })
    }

    /// Registers canonical physical/application source identities resolved by the adapter.
    /// Different aliases for the same microphone/application must resolve to this same ID.
    /// Local gain/channel choices belong to references, not duplicated native owners.
    /// # Errors
    /// Rejects duplicate IDs, non-source configs, non-unity owner gain and invalid widths.
    pub fn with_sources(mut self, sources: Vec<SharedSource>) -> Result<Self, CompositionError> {
        if sources.len() > MAX_IDENTITIES.saturating_sub(self.sources.len()) {
            return Err(invalid("too many shared capture sources"));
        }
        for source in sources {
            validate_id(&source.id)?;
            if let NodeConfig::ApplicationCapture(request) = &source.config {
                request
                    .validate_contract(64)
                    .map_err(|error| invalid(error.to_string()))?;
                if let Some(id) = &request.forwarding_endpoint {
                    let device = self
                        .get(id)
                        .ok_or_else(|| invalid("unknown application forwarding endpoint"))?;
                    if device.render.is_none() {
                        return Err(invalid(
                            "application forwarding endpoint has no render stream",
                        ));
                    }
                }
            }
            let (channels, gain) = source_shape(&source.config)?;
            if channels == 0 || channels > 64 || gain != 0.0 {
                return Err(invalid("invalid canonical source channels/gain"));
            }
            if self.sources.insert(source.id, source.config).is_some() {
                return Err(invalid("duplicate shared capture source identity"));
            }
        }
        Ok(self)
    }

    /// Registers adapter-resolved physical sinks; graph references never own the native sink.
    /// # Errors
    /// Rejects duplicate/invalid stable identities and unsupported canonical channel counts.
    pub fn with_outputs(mut self, outputs: Vec<SharedOutput>) -> Result<Self, CompositionError> {
        if outputs.len() > MAX_IDENTITIES.saturating_sub(self.outputs.len()) {
            return Err(invalid("too many shared physical outputs"));
        }
        for output in outputs {
            validate_id(&output.id)?;
            if !(1..=64).contains(&output.config.input_channels) {
                return Err(invalid("invalid canonical output channel count"));
            }
            if self.outputs.insert(output.id, output.config).is_some() {
                return Err(invalid("duplicate shared physical output identity"));
            }
        }
        Ok(self)
    }

    /// Iterates the canonical virtual specifications, including currently unreferenced devices.
    pub fn devices(&self) -> impl Iterator<Item = &VirtualEndpointSpec> {
        self.devices.values()
    }

    #[must_use]
    /// Looks up an exact virtual device identity, without a default-device fallback.
    pub fn get(&self, id: &VirtualEndpointId) -> Option<&VirtualEndpointSpec> {
        self.devices.get(id)
    }
}

/// Canonical native capture configuration with graph-independent zero gain.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedSource {
    pub id: String,
    pub config: NodeConfig,
}

/// Graph-local channel projection and gain of one canonical capture source.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceReference {
    pub control: crate::NodeControl,
    pub node_id: NodeId,
    pub source_id: String,
    pub channels: u16,
    pub gain_db: f32,
    /// Graph-local render endpoint to suppress while this source's mask is enabled.
    pub forwarding_endpoint: Option<VirtualEndpointId>,
}

/// One authored, enabled application's playback intent; capture ownership stays canonical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationPlaybackIntent {
    /// Adapter-resolved canonical capture identity.
    pub source_id: String,
    /// Actual native node bound to that capture identity in the prepared graph.
    pub capture_node_id: NodeId,
    /// Namespaced graph-local runtime control mask.
    pub mask_node_id: NodeId,
    /// Explicit app-owned render endpoint, never a system default.
    pub forwarding_endpoint: VirtualEndpointId,
}

/// Canonical physical sink whose stable identity was resolved by its native adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedOutput {
    pub id: String,
    pub config: crate::AudioDeviceOutputConfig,
}

/// Graph-local sink routing and gain; all references sum into one canonical native sink.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputReference {
    pub control: crate::NodeControl,
    pub node_id: NodeId,
    pub output_id: String,
    pub channels: u16,
    pub gain_db: f32,
}

/// Requested graph-local width of the combined device source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceReadReference {
    pub channels: u16,
}

/// A graph-local view of a Driver-owned device. No copied name/channel/rate definition.
/// The mixed capture snapshot uses `from_os`; graph writes enter Driver render
/// via `to_os`. Snapshot publication follows the device I/O cycle, not feedback detection.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceReference {
    pub control: crate::NodeControl,
    pub node_id: NodeId,
    pub device_id: VirtualEndpointId,
    pub read: Option<DeviceReadReference>,
    pub write_channels: Option<u16>,
    pub gain_db: f32,
}

/// `document.nodes` contains processors. Native input/output and virtual-device references
/// are supplied separately; authored edges use their local IDs. `lower_document` converts
/// the current authored document using adapter-resolved identities before composition.
pub struct GraphFragment<'a> {
    pub id: &'a str,
    pub document: &'a GraphDocument,
    pub device_references: &'a [DeviceReference],
    pub source_references: &'a [SourceReference],
    pub output_references: &'a [OutputReference],
}

/// Projection from a namespaced runtime meter/control to its graph-local identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeOrigin {
    pub graph_id: String,
    pub node_id: NodeId,
    /// A generated reference gain uses `audio` internally; project its meter back to
    /// this authored logical port. Ordinary processor ports remain unchanged.
    pub meter_port: Option<PortId>,
}

/// Executable graph plus stable native bindings and graph-local meter/control projections.
pub struct ComposedGraph {
    pub document: GraphDocument,
    pub execution: ExecutionPlan,
    pub node_origins: BTreeMap<NodeId, NodeOrigin>,
    /// Shared runtime owners and the graph-local masks that require them. An owner is
    /// enabled when any listed mask is enabled; mute and gain remain graph-local.
    /// Virtual transport, mixer and snapshot owners share the same reference masks.
    pub native_control_bindings: BTreeMap<NodeId, Vec<NodeId>>,
    pub native_devices: BTreeMap<VirtualEndpointId, NodeId>,
    pub device_buses: BTreeMap<VirtualEndpointId, NodeId>,
    pub native_sources: BTreeMap<String, NodeId>,
    pub native_outputs: BTreeMap<String, NodeId>,
    /// Graph-local mute intents evaluated against the same plan control bank.
    pub playback_intents: Vec<ApplicationPlaybackIntent>,
}

/// Failure to validate references or compile their combined topology.
#[derive(Debug, thiserror::Error)]
pub enum CompositionError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Compile(CompileError),
}

/// Namespaces graph-local processor/control identity without delimiter collisions.
#[must_use]
pub fn runtime_node_id(graph_id: &str, node_id: &NodeId) -> NodeId {
    NodeId::new(format!(
        "g:{}:{graph_id}:n:{}:{node_id}",
        graph_id.len(),
        node_id.as_str().len()
    ))
}

/// Compiles all supplied running graphs, independent of which one the UI is displaying.
/// Native endpoint rates remain canonical; adapters must bridge the chosen DSP domain.
/// # Errors
/// Rejects invalid references, duplicate/local identities, exceeded aggregate budgets,
/// missing channels, and processor-only cycles without an I/O time boundary.
///
/// # Example
/// ```
/// use mixrelay_audio_graph::composition::{DeviceRegistry, compose};
/// use mixrelay_audio_graph::{CompileContext, CompileLimits, GraphCompiler, RenderFormat};
/// let registry = DeviceRegistry::new(Vec::new())?;
/// let context = CompileContext {
///     format: RenderFormat { sample_rate: 48_000, block_frames: 256 },
///     limits: CompileLimits::default(),
/// };
/// let combined = compose(&registry, &[], 1, context, &GraphCompiler::default())?;
/// assert!(combined.native_outputs.is_empty());
/// # Ok::<(), mixrelay_audio_graph::composition::CompositionError>(())
/// ```
pub fn compose(
    registry: &DeviceRegistry,
    graphs: &[GraphFragment<'_>],
    revision: u64,
    context: CompileContext,
    compiler: &GraphCompiler,
) -> Result<ComposedGraph, CompositionError> {
    if graphs.len() > MAX_IDENTITIES {
        return Err(invalid("too many running graphs"));
    }
    let mut state = lowering::Builder::new(registry, graphs, revision, context)?;
    let mut identities = BTreeSet::new();
    for graph in graphs {
        validate_id(graph.id)?;
        if !identities.insert(graph.id) {
            return Err(invalid("duplicate running graph identity"));
        }
        state.add_graph(graph)?;
    }
    state.finish(compiler)
}

fn validate_id(value: &str) -> Result<(), CompositionError> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(invalid("invalid graph/device/local identity"))
    } else {
        Ok(())
    }
}
fn invalid(reason: impl Into<String>) -> CompositionError {
    CompositionError::Invalid(reason.into())
}

fn reference_node_id(graph: &str, node: &NodeId, side: &str) -> NodeId {
    NodeId::new(format!(
        "g:{}:{graph}:r:{}:{node}:{side}",
        graph.len(),
        node.as_str().len()
    ))
}
fn device_node_id(id: &VirtualEndpointId, side: &str) -> NodeId {
    NodeId::new(format!("d:{}:{}:{side}", id.as_str().len(), id.as_str()))
}
fn source_node_id(id: &str) -> NodeId {
    NodeId::new(format!("s:{}:{id}:native", id.len()))
}
fn output_node_id(id: &str) -> NodeId {
    NodeId::new(format!("o:{}:{id}:native", id.len()))
}
fn source_shape(config: &NodeConfig) -> Result<(u16, f32), CompositionError> {
    match config {
        NodeConfig::AudioDeviceInput(value) => Ok((value.output_channels, value.gain_db)),
        NodeConfig::ApplicationCapture(value) => Ok((value.stream.channels, value.gain_db)),
        _ => Err(invalid(
            "canonical source must be a physical input or application capture",
        )),
    }
}
fn endpoint(node: NodeId, port: &str) -> AudioEndpoint {
    AudioEndpoint {
        node_id: node,
        port_id: PortId::from(port),
    }
}
