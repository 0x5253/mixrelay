use super::{
    BTreeMap, BTreeSet, CompositionError, DeviceReadReference, DeviceReference, DeviceRegistry,
    GraphDocument, GraphFragment, NodeConfig, NodeId, NodeSpec, OutputReference, SourceReference,
    invalid, source_shape, validate_id,
};

/// Control-plane intermediate produced from the current authored document; never persisted
/// as a second schema. Native identities and presentation labels remain separate.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthoredGraph {
    pub document: GraphDocument,
    pub device_references: Vec<DeviceReference>,
    pub source_references: Vec<SourceReference>,
    pub output_references: Vec<OutputReference>,
    /// Reference labels are presentation data, not native source/device identity.
    pub reference_labels: BTreeMap<NodeId, Option<String>>,
}

impl AuthoredGraph {
    /// Borrows this lowered input under its catalog-owned graph identity.
    #[must_use]
    pub fn as_fragment<'a>(&'a self, graph_id: &'a str) -> GraphFragment<'a> {
        GraphFragment {
            id: graph_id,
            document: &self.document,
            device_references: &self.device_references,
            source_references: &self.source_references,
            output_references: &self.output_references,
        }
    }
}

/// Native adapter identity resolutions for each authored capture and physical sink node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedNodeIdentities {
    pub sources: BTreeMap<NodeId, String>,
    pub outputs: BTreeMap<NodeId, String>,
}

/// Lowers current authored native nodes into references using adapter-resolved identities.
/// `identities` must come from actual device/application identity resolution, never labels
/// or guessed equivalence of selectors. For example, a default input and its persistent
/// UID can map to one source only after the adapter resolves the default selection.
/// # Errors
/// Rejects absent mappings, incompatible canonical sources/stream widths, duplicate local
/// IDs and policies incompatible with the shared native owner.
pub fn lower_document(
    document: &GraphDocument,
    registry: &DeviceRegistry,
    identities: &ResolvedNodeIdentities,
) -> Result<AuthoredGraph, CompositionError> {
    let mut lowered = AuthoredGraph {
        document: GraphDocument::new(document.revision, vec![], document.edges.clone()),
        device_references: vec![],
        source_references: vec![],
        output_references: vec![],
        reference_labels: BTreeMap::new(),
    };
    lowered.document.schema_version = document.schema_version;
    let mut ids = BTreeSet::new();
    for node in &document.nodes {
        validate_id(node.id.as_str())?;
        if !ids.insert(&node.id) {
            return Err(invalid("duplicate node in authored input"));
        }
        match &node.config {
            NodeConfig::ApplicationCapture(request) if request.requires_reselection => {
                request
                    .validate_contract(64)
                    .map_err(|error| invalid(error.to_string()))?;
                if document
                    .edges
                    .iter()
                    .any(|edge| edge.destination.node_id == node.id)
                {
                    return Err(invalid("application capture cannot receive graph audio"));
                }
                // Keep authored ports/edges editable without resolving a stale PID. An
                // unconnected local gain produces silence and owns no native source.
                lowered.document.nodes.push(NodeSpec {
                    control: node.control,
                    id: node.id.clone(),
                    label: node.label.clone(),
                    config: NodeConfig::Gain(crate::GainConfig {
                        channels: request.stream.channels,
                        gain_db: 0.0,
                    }),
                });
                continue;
            }
            NodeConfig::AudioDeviceInput(_) | NodeConfig::ApplicationCapture(_) => {
                lowered
                    .source_references
                    .push(lower_source(node, registry, &identities.sources)?);
            }
            NodeConfig::AudioDeviceOutput(config) => {
                lowered
                    .output_references
                    .push(lower_output(node, config, registry, identities)?);
            }
            NodeConfig::VirtualEndpoint(config) => {
                lowered
                    .device_references
                    .push(lower_device(node, config, registry)?);
            }
            _ => {
                lowered.document.nodes.push(node.clone());
                continue;
            }
        }
        lowered
            .reference_labels
            .insert(node.id.clone(), node.label.clone());
    }
    Ok(lowered)
}

fn lower_output(
    node: &NodeSpec,
    config: &crate::AudioDeviceOutputConfig,
    registry: &DeviceRegistry,
    identities: &ResolvedNodeIdentities,
) -> Result<OutputReference, CompositionError> {
    let node_id = &node.id;
    let output_id = identities
        .outputs
        .get(node_id)
        .ok_or_else(|| invalid("physical output requires resolved canonical identity"))?;
    let canonical = registry
        .outputs
        .get(output_id)
        .ok_or_else(|| invalid("physical output identity is not registered"))?;
    if config.input_channels == 0 || config.input_channels > canonical.input_channels {
        return Err(invalid(
            "physical output reference exceeds canonical channel count",
        ));
    }
    Ok(OutputReference {
        control: node.control,
        node_id: node_id.clone(),
        output_id: output_id.clone(),
        channels: config.input_channels,
        gain_db: 0.0,
    })
}

fn lower_device(
    node: &NodeSpec,
    config: &crate::VirtualEndpointConfig,
    registry: &DeviceRegistry,
) -> Result<DeviceReference, CompositionError> {
    let node_id = &node.id;
    crate::validate_virtual_endpoint_gain(config.gain_db)
        .map_err(|error| invalid(error.to_string()))?;
    let canonical = registry
        .get(&config.id)
        .ok_or_else(|| invalid("references require registered virtual device"))?;
    if config.render.is_some_and(|stream| {
        canonical
            .render
            .is_none_or(|value| stream.channels > value.channels)
    }) || config.capture.is_some_and(|stream| {
        canonical
            .capture
            .is_none_or(|value| stream.channels > value.channels)
    }) {
        return Err(invalid(
            "virtual reference exceeds canonical stream channels",
        ));
    }
    if !config.reference.render && !config.reference.capture {
        return Err(invalid(
            "virtual reference must expose at least one signal direction",
        ));
    }
    let read = if config.reference.render {
        let channels = config
            .capture
            .ok_or_else(|| invalid("authored virtual source has no capture stream"))?
            .channels;
        Some(DeviceReadReference { channels })
    } else {
        None
    };
    let write_channels = if config.reference.capture {
        Some(
            config
                .render
                .ok_or_else(|| invalid("authored virtual write has no render stream"))?
                .channels,
        )
    } else {
        None
    };
    Ok(DeviceReference {
        control: node.control,
        node_id: node_id.clone(),
        device_id: config.id.clone(),
        read,
        write_channels,
        gain_db: config.gain_db,
    })
}

fn lower_source(
    node: &NodeSpec,
    registry: &DeviceRegistry,
    source_ids: &BTreeMap<NodeId, String>,
) -> Result<SourceReference, CompositionError> {
    let source_id = source_ids
        .get(&node.id)
        .ok_or_else(|| invalid("references require resolved canonical source identity"))?;
    let canonical = registry
        .sources
        .get(source_id)
        .ok_or_else(|| invalid("references an unregistered canonical source"))?;
    match (&node.config, canonical) {
        (NodeConfig::AudioDeviceInput(_), NodeConfig::AudioDeviceInput(_)) => {}
        (NodeConfig::ApplicationCapture(local), NodeConfig::ApplicationCapture(owner)) => {
            local
                .validate_contract(64)
                .map_err(|error| invalid(error.to_string()))?;
            if local.requires_reselection != owner.requires_reselection {
                return Err(invalid(
                    "references require explicit application policy resolution",
                ));
            }
            if let Some(id) = &local.forwarding_endpoint {
                let endpoint = registry
                    .get(id)
                    .ok_or_else(|| invalid("unknown application forwarding endpoint"))?;
                if endpoint.render.is_none() {
                    return Err(invalid(
                        "application forwarding endpoint has no render stream",
                    ));
                }
            }
        }
        _ => {
            return Err(invalid(
                "authored source kind does not match canonical identity",
            ));
        }
    }
    let (channels, gain_db) = source_shape(&node.config)?;
    let (canonical_channels, _) = source_shape(canonical)?;
    if channels == 0 || channels > canonical_channels || !gain_db.is_finite() {
        return Err(invalid("authored source channels/gain are invalid"));
    }
    Ok(SourceReference {
        control: node.control,
        node_id: node.id.clone(),
        source_id: source_id.clone(),
        channels,
        gain_db,
        forwarding_endpoint: match &node.config {
            NodeConfig::ApplicationCapture(request) => request.forwarding_endpoint.clone(),
            _ => None,
        },
    })
}
