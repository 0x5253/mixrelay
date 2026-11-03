//! Control-plane lowering of every catalog document into one running DSP composition.
//! Native inventory and verified application identity enter through explicit boundaries;
//! editor selection, display labels and guessed default-device equivalence do not.

use crate::graph_composition::{
    ComposedGraph, CompositionError, DeviceRegistry, ResolvedNodeIdentities, SharedOutput,
    SharedSource, compose, lower_document,
};
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, CompileContext, GraphCompiler, GraphDocument,
    NodeConfig, NodeSpec, VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_audio_io::{
    ApplicationCaptureRequest, AudioDeviceDescriptor, DeviceDirection, DeviceSelector,
};
use std::collections::BTreeMap;

/// Adapter-owned application identity resolution. The canonical request must have zero
/// gain and an actual supported channel width. Different aliases may share an ID only
/// after native resolution establishes that they capture the same signal. Playback
/// policy remains a graph-local control intent, never a second capture owner.
pub trait ApplicationIdentityResolver {
    /// Resolves one authored request without opening another capture owner.
    /// # Errors
    /// Rejects unavailable/ambiguous identity, unsupported policy and stale PID selections.
    fn resolve(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<SharedSource, CompositionError>;
}

/// One coherent control-plane input snapshot, independent of any selected editor graph.
#[derive(Clone, Copy)]
pub struct CatalogExecutionContext<'a> {
    pub revision: u64,
    pub compile: CompileContext,
    pub compiler: &'a GraphCompiler,
    /// Actual native device inventory, e.g. the result of macOS device enumeration.
    pub devices: &'a [AudioDeviceDescriptor],
    pub applications: &'a dyn ApplicationIdentityResolver,
}

/// Composes all supplied catalog documents without mutating their authored contents.
/// Empty catalogs produce an empty plan; no default device or source is synthesized.
/// # Errors
/// Rejects conflicting device definitions, ambiguous physical/application identities,
/// incompatible shared-owner policies and invalid references. Absent physical selections
/// remain native bindings so adapters can silence and reconnect them independently.
pub fn compose_catalog(
    documents: &[(String, GraphDocument)],
    context: CatalogExecutionContext<'_>,
) -> Result<ComposedGraph, CompositionError> {
    validate_size(documents, context.compile)?;
    let documents = documents
        .iter()
        .map(|(id, document)| Ok((id.clone(), execution_document(document)?)))
        .collect::<Result<Vec<_>, CompositionError>>()?;
    let mut registry = RegistryInputs::default();
    let mut identities = Vec::with_capacity(documents.len());
    for (_, document) in &documents {
        identities.push(registry.resolve(document, &context)?);
    }
    let registry = DeviceRegistry::new(registry.devices.into_values().collect())?
        .with_sources(registry.sources.into_values().collect())?
        .with_outputs(registry.outputs.into_values().collect())?;
    let authored = documents
        .iter()
        .zip(&identities)
        .map(|((_, document), identities)| lower_document(document, &registry, identities))
        .collect::<Result<Vec<_>, _>>()?;
    let fragments = documents
        .iter()
        .zip(&authored)
        .map(|((id, _), authored)| authored.as_fragment(id))
        .collect::<Vec<_>>();
    compose(
        &registry,
        &fragments,
        context.revision,
        context.compile,
        context.compiler,
    )
}

fn execution_document(document: &GraphDocument) -> Result<GraphDocument, CompositionError> {
    let mut ids = std::collections::BTreeSet::new();
    for node in &document.nodes {
        if !ids.insert(&node.id) {
            return Err(invalid("duplicate authored node identity"));
        }
        if matches!(
            node.config,
            NodeConfig::VirtualEndpointTransport(_) | NodeConfig::DeviceBus { .. }
        ) {
            return Err(invalid("internal runtime node cannot be authored"));
        }
    }
    if document
        .edges
        .iter()
        .any(|edge| !ids.contains(&edge.source.node_id) || !ids.contains(&edge.destination.node_id))
    {
        return Err(invalid("edge references unknown authored node"));
    }
    let mut prepared = document.clone();
    for node in &mut prepared.nodes {
        if !node.control.enabled
            && let NodeConfig::ApplicationCapture(request) = &mut node.config
        {
            // Keep a dormant graph-local slot without resolving an OFF process or
            // prompting for native capture. First activation explicitly prepares it.
            request.requires_reselection = true;
            request.source_playback = mixrelay_audio_io::SourcePlaybackPolicy::Preserve;
            request.forwarding_endpoint = None;
        }
    }
    Ok(prepared)
}

fn validate_size(
    documents: &[(String, GraphDocument)],
    context: CompileContext,
) -> Result<(), CompositionError> {
    let nodes = documents.iter().try_fold(0_usize, |total, (_, graph)| {
        total.checked_add(graph.nodes.len())
    });
    let edges = documents.iter().try_fold(0_usize, |total, (_, graph)| {
        total.checked_add(graph.edges.len())
    });
    if documents.len() > 256
        || nodes.is_none_or(|count| count > context.limits.max_nodes)
        || edges.is_none_or(|count| count > context.limits.max_edges)
    {
        return Err(invalid(
            "catalog exceeds aggregate graph compilation limits",
        ));
    }
    Ok(())
}

#[derive(Default)]
struct RegistryInputs {
    devices: BTreeMap<VirtualEndpointId, VirtualEndpointSpec>,
    sources: BTreeMap<String, SharedSource>,
    outputs: BTreeMap<String, SharedOutput>,
}
impl RegistryInputs {
    fn resolve(
        &mut self,
        document: &GraphDocument,
        context: &CatalogExecutionContext<'_>,
    ) -> Result<ResolvedNodeIdentities, CompositionError> {
        let mut identities = ResolvedNodeIdentities::default();
        for node in &document.nodes {
            match &node.config {
                NodeConfig::VirtualEndpoint(config) => {
                    insert_same(&mut self.devices, config.id.clone(), config.spec.clone())?;
                }
                NodeConfig::ApplicationCapture(request) if request.requires_reselection => {
                    // A restored process identifier is intentionally disconnected until
                    // the user selects a live process. It owns no native capture resource.
                }
                NodeConfig::AudioDeviceInput(_) | NodeConfig::ApplicationCapture(_) => {
                    let source = resolve_source(node, context)?;
                    identities
                        .sources
                        .insert(node.id.clone(), source.id.clone());
                    self.insert_source(source)?;
                }
                NodeConfig::AudioDeviceOutput(config) => {
                    let output = resolve_output(config, context.devices)?;
                    identities
                        .outputs
                        .insert(node.id.clone(), output.id.clone());
                    self.insert_output(output)?;
                }
                _ => {}
            }
        }
        Ok(identities)
    }

    fn insert_source(&mut self, mut source: SharedSource) -> Result<(), CompositionError> {
        if let Some(existing) = self.sources.get_mut(&source.id)
            && let (NodeConfig::AudioDeviceInput(current), NodeConfig::AudioDeviceInput(next)) =
                (&mut existing.config, &mut source.config)
            && current.selector == next.selector
        {
            // Missing devices have no observed width. Reserve the widest authored
            // request once, while each graph keeps its own channel projection.
            current.output_channels = current.output_channels.max(next.output_channels);
            return Ok(());
        }
        if let Some(existing) = self.sources.get_mut(&source.id)
            && let (NodeConfig::ApplicationCapture(current), NodeConfig::ApplicationCapture(next)) =
                (&mut existing.config, &source.config)
        {
            let mut projected = current.clone();
            projected.stream = next.stream;
            if projected == *next {
                if current.stream.channels < next.stream.channels {
                    current.stream = next.stream;
                }
                return Ok(());
            }
        }
        insert_same(&mut self.sources, source.id.clone(), source)
    }

    fn insert_output(&mut self, output: SharedOutput) -> Result<(), CompositionError> {
        if let Some(existing) = self.outputs.get_mut(&output.id)
            && existing.config.selector == output.config.selector
        {
            existing.config.input_channels = existing
                .config
                .input_channels
                .max(output.config.input_channels);
            return Ok(());
        }
        insert_same(&mut self.outputs, output.id.clone(), output)
    }
}

fn resolve_source(
    node: &NodeSpec,
    context: &CatalogExecutionContext<'_>,
) -> Result<SharedSource, CompositionError> {
    match &node.config {
        NodeConfig::AudioDeviceInput(config) => {
            let (id, selector, channels) = resolve_physical(
                &config.selector,
                context.devices,
                DeviceDirection::Input,
                config.output_channels,
            )?;
            Ok(SharedSource {
                id,
                config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector,
                    output_channels: channels,
                    gain_db: 0.0,
                }),
            })
        }
        NodeConfig::ApplicationCapture(request) => {
            let mut resolved = context.applications.resolve(request)?;
            let NodeConfig::ApplicationCapture(canonical) = &mut resolved.config else {
                return Err(invalid(
                    "application resolver returned a non-application source",
                ));
            };
            // The adapter validated the original authored policy. The shared native tap
            // always captures pre-mute PCM; playback intent is retained on each reference.
            canonical.source_playback = mixrelay_audio_io::SourcePlaybackPolicy::Preserve;
            canonical.forwarding_endpoint = None;
            resolved.id = format!("application:{}:{}", resolved.id.len(), resolved.id);
            Ok(resolved)
        }
        _ => Err(invalid("node is not a native capture source")),
    }
}

fn resolve_output(
    config: &AudioDeviceOutputConfig,
    devices: &[AudioDeviceDescriptor],
) -> Result<SharedOutput, CompositionError> {
    let (id, selector, channels) = resolve_physical(
        &config.selector,
        devices,
        DeviceDirection::Output,
        config.input_channels,
    )?;
    Ok(SharedOutput {
        id,
        config: AudioDeviceOutputConfig {
            selector,
            input_channels: channels,
        },
    })
}

fn resolve_physical(
    selector: &DeviceSelector,
    devices: &[AudioDeviceDescriptor],
    direction: DeviceDirection,
    channels: u16,
) -> Result<(String, DeviceSelector, u16), CompositionError> {
    let resolved = selector.resolve(devices, direction, channels);
    if let Ok(device) = resolved {
        let width = match direction {
            DeviceDirection::Input => device.input_channels,
            DeviceDirection::Output => device.output_channels,
        };
        return Ok((
            physical_id(&device.uid),
            DeviceSelector::PersistentUid {
                uid: device.uid.clone(),
            },
            supported_width(width)?,
        ));
    }
    // Zero channels in this direction means that IO has disappeared even if the
    // opposite direction remains online. A nonzero but insufficient width is invalid.
    if !physical_io_absent(selector, devices, direction) {
        return Err(invalid(
            "physical selection is ambiguous or has insufficient channels",
        ));
    }
    let missing_id = match (selector, direction) {
        (DeviceSelector::PersistentUid { uid }, _) if !uid.is_empty() => Some(physical_id(uid)),
        (DeviceSelector::DefaultInput, DeviceDirection::Input) => {
            Some("unresolved-default:input".into())
        }
        (DeviceSelector::DefaultOutput, DeviceDirection::Output) => {
            Some("unresolved-default:output".into())
        }
        _ => None,
    };
    if (1..=64).contains(&channels)
        && let Some(id) = missing_id
    {
        return Ok((id, selector.clone(), channels));
    }
    Err(invalid(
        "physical selection is ambiguous or has invalid direction/channel width",
    ))
}

fn physical_io_absent(
    selector: &DeviceSelector,
    devices: &[AudioDeviceDescriptor],
    direction: DeviceDirection,
) -> bool {
    let mut candidates = devices.iter().filter(|device| match selector {
        DeviceSelector::PersistentUid { uid } => &device.uid == uid,
        DeviceSelector::DefaultInput => device.is_default_input,
        DeviceSelector::DefaultOutput => device.is_default_output,
    });
    let first = candidates.next();
    candidates.next().is_none()
        && first.is_none_or(|device| match direction {
            DeviceDirection::Input => device.input_channels == 0,
            DeviceDirection::Output => device.output_channels == 0,
        })
}

fn physical_id(uid: &str) -> String {
    format!("device:{}:{uid}", uid.len())
}

fn supported_width(actual: u32) -> Result<u16, CompositionError> {
    // A wide interface remains one native owner, with the product's first 64 channels
    // available to references; a requested width was already checked against reality.
    u16::try_from(actual.min(64))
        .ok()
        .filter(|width| *width != 0)
        .ok_or_else(|| invalid("native device has no supported channels"))
}

fn insert_same<K: Ord, V: PartialEq>(
    entries: &mut BTreeMap<K, V>,
    key: K,
    value: V,
) -> Result<(), CompositionError> {
    match entries.entry(key) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(value);
        }
        std::collections::btree_map::Entry::Occupied(entry) if entry.get() != &value => {
            return Err(invalid(
                "conflicting canonical definitions for one native identity",
            ));
        }
        std::collections::btree_map::Entry::Occupied(_) => {}
    }
    Ok(())
}

fn invalid(reason: impl Into<String>) -> CompositionError {
    CompositionError::Invalid(reason.into())
}

#[cfg(test)]
#[path = "catalog_execution_tests.rs"]
mod tests;
