use super::{
    AUDIO_PORT_ID, ApplicationPlaybackIntent, AudioEndpoint, BTreeMap, BTreeSet, CompileContext,
    ComposedGraph, CompositionError, DeviceReference, DeviceRegistry, EdgeId, EdgeSpec,
    FROM_OS_PORT_ID, GainConfig, GraphCompiler, GraphDocument, GraphFragment, MixerConfig,
    NodeConfig, NodeId, NodeOrigin, NodeSpec, OutputReference, PortId, SourceReference,
    TO_OS_PORT_ID, VirtualEndpointId, device_node_id, endpoint, invalid, output_node_id,
    reference_node_id, runtime_node_id, source_node_id, source_shape, validate_id,
};

#[derive(Default)]
struct ClientDirections {
    read: bool,
    write: bool,
}

pub(super) struct Builder<'a> {
    registry: &'a DeviceRegistry,
    context: CompileContext,
    document: GraphDocument,
    origins: BTreeMap<NodeId, NodeOrigin>,
    control_bindings: BTreeMap<NodeId, Vec<NodeId>>,
    devices: BTreeMap<VirtualEndpointId, NodeId>,
    buses: BTreeMap<VirtualEndpointId, NodeId>,
    sources: BTreeMap<String, NodeId>,
    outputs: BTreeMap<String, NodeId>,
    playback_intents: Vec<ApplicationPlaybackIntent>,
}

impl<'a> Builder<'a> {
    pub(super) fn new(
        registry: &'a DeviceRegistry,
        graphs: &[GraphFragment<'_>],
        revision: u64,
        context: CompileContext,
    ) -> Result<Self, CompositionError> {
        let mut value = Self {
            registry,
            context,
            document: GraphDocument::new(revision, vec![], vec![]),
            origins: BTreeMap::new(),
            control_bindings: BTreeMap::new(),
            devices: BTreeMap::new(),
            buses: BTreeMap::new(),
            sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            playback_intents: Vec::new(),
        };
        let mut referenced: BTreeMap<_, ClientDirections> = BTreeMap::new();
        for reference in graphs.iter().flat_map(|graph| graph.device_references) {
            let directions = referenced.entry(&reference.device_id).or_default();
            directions.read |= reference.read.is_some();
            directions.write |= reference.write_channels.is_some();
        }
        for reference in graphs.iter().flat_map(|graph| graph.source_references) {
            if let Some(id) = &reference.forwarding_endpoint {
                referenced.entry(id).or_default().write = true;
            }
        }
        for (id, directions) in referenced {
            let mut device = registry
                .get(id)
                .ok_or_else(|| invalid("unknown shared device"))?
                .clone();
            if !directions.read {
                device.capture = None;
            }
            if !directions.write {
                device.render = None;
            }
            value.device(&device)?;
        }
        Ok(value)
    }

    fn device(&mut self, device: &crate::VirtualEndpointSpec) -> Result<(), CompositionError> {
        let native = device_node_id(&device.id, "native");
        self.node(NodeSpec {
            id: native.clone(),
            control: crate::NodeControl::default(),
            label: Some(device.display_name.clone()),
            config: NodeConfig::VirtualEndpointTransport(device.clone()),
        })?;
        self.devices.insert(device.id.clone(), native.clone());
        if let Some(render) = device.render {
            let mix = device_node_id(&device.id, "mix");
            self.node(NodeSpec {
                id: mix.clone(),
                control: crate::NodeControl::default(),
                label: None,
                config: NodeConfig::Mixer(MixerConfig {
                    inputs: vec![PortId::from(AUDIO_PORT_ID)],
                    channels: render.channels,
                }),
            })?;
            self.project_channels(
                endpoint(mix, AUDIO_PORT_ID),
                endpoint(native.clone(), TO_OS_PORT_ID),
                render.channels,
            )?;
        }
        if let Some(capture) = device.capture {
            let snapshot = device_node_id(&device.id, "snapshot");
            self.node(NodeSpec {
                id: snapshot.clone(),
                control: crate::NodeControl::default(),
                label: None,
                config: NodeConfig::DeviceBus {
                    channels: capture.channels,
                },
            })?;
            self.link(
                endpoint(native, FROM_OS_PORT_ID),
                endpoint(snapshot.clone(), AUDIO_PORT_ID),
            )?;
            self.buses.insert(device.id.clone(), snapshot);
        }
        Ok(())
    }

    pub(super) fn add_graph(&mut self, graph: &GraphFragment<'_>) -> Result<(), CompositionError> {
        if graph.document.schema_version != crate::CURRENT_GRAPH_SCHEMA_VERSION {
            return Err(invalid("unsupported authored graph schema"));
        }
        let mut nodes = self.processor_nodes(graph)?;
        let mut source_references = BTreeSet::new();
        for reference in graph.source_references {
            validate_id(reference.node_id.as_str())?;
            if !nodes.insert(reference.node_id.clone()) {
                return Err(invalid("duplicate graph-local source reference"));
            }
            source_references.insert(reference.node_id.clone());
            self.source_reference(graph.id, reference)?;
        }
        let mut references = BTreeMap::new();
        for reference in graph.device_references {
            validate_id(reference.node_id.as_str())?;
            if !nodes.insert(reference.node_id.clone()) {
                return Err(invalid("duplicate graph-local device reference"));
            }
            references.insert(reference.node_id.clone(), reference);
            self.reference(graph.id, reference)?;
        }
        let mut edges = BTreeSet::new();
        let outputs = self.output_references(graph, &mut nodes)?;
        for edge in &graph.document.edges {
            if source_references.contains(&edge.destination.node_id)
                || outputs.contains(&edge.source.node_id)
            {
                return Err(invalid("edge reverses a native reference direction"));
            }
            validate_id(edge.id.as_str())?;
            if !edges.insert(&edge.id) {
                return Err(invalid("duplicate graph-local edge identity"));
            }
            let source = Self::map_endpoint(graph.id, &edge.source, true, &nodes, &references)?;
            let destination =
                Self::map_endpoint(graph.id, &edge.destination, false, &nodes, &references)?;
            self.edge(EdgeSpec {
                id: EdgeId::new(format!(
                    "g:{}:{}:e:{}:{}",
                    graph.id.len(),
                    graph.id,
                    edge.id.as_str().len(),
                    edge.id
                )),
                source,
                destination,
                channel_mapping: edge.channel_mapping.clone(),
            })?;
        }
        Ok(())
    }

    fn processor_nodes(
        &mut self,
        graph: &GraphFragment<'_>,
    ) -> Result<BTreeSet<NodeId>, CompositionError> {
        let mut nodes = BTreeSet::new();
        for node in &graph.document.nodes {
            validate_id(node.id.as_str())?;
            if !nodes.insert(node.id.clone()) {
                return Err(invalid("duplicate graph-local node identity"));
            }
            if matches!(
                node.config,
                NodeConfig::VirtualEndpoint(_)
                    | NodeConfig::VirtualEndpointTransport(_)
                    | NodeConfig::DeviceBus { .. }
                    | NodeConfig::AudioDeviceInput(_)
                    | NodeConfig::AudioDeviceOutput(_)
                    | NodeConfig::ApplicationCapture(_)
            ) {
                return Err(invalid(
                    "native nodes require resolved registry references before composition",
                ));
            }
            let mut lowered = node.clone();
            lowered.id = runtime_node_id(graph.id, &node.id);
            self.origin(lowered.id.clone(), graph.id, &node.id, None);
            self.node(lowered)?;
        }
        Ok(nodes)
    }

    fn output_references(
        &mut self,
        graph: &GraphFragment<'_>,
        nodes: &mut BTreeSet<NodeId>,
    ) -> Result<BTreeSet<NodeId>, CompositionError> {
        let mut outputs = BTreeSet::new();
        for reference in graph.output_references {
            validate_id(reference.node_id.as_str())?;
            if !nodes.insert(reference.node_id.clone()) {
                return Err(invalid("duplicate graph-local output reference"));
            }
            self.output_reference(graph.id, reference)?;
            outputs.insert(reference.node_id.clone());
        }
        Ok(outputs)
    }

    fn output_reference(
        &mut self,
        graph: &str,
        reference: &OutputReference,
    ) -> Result<(), CompositionError> {
        let config = self
            .registry
            .outputs
            .get(&reference.output_id)
            .ok_or_else(|| invalid("unknown shared physical output"))?;
        if reference.channels == 0
            || reference.channels > config.input_channels
            || !reference.gain_db.is_finite()
        {
            return Err(invalid("invalid output reference channels/gain"));
        }
        let native = output_node_id(&reference.output_id);
        if !self.outputs.contains_key(&reference.output_id) {
            self.node(NodeSpec {
                id: native.clone(),
                control: crate::NodeControl::default(),
                label: None,
                config: NodeConfig::AudioDeviceOutput(config.clone()),
            })?;
            self.outputs
                .insert(reference.output_id.clone(), native.clone());
        }
        let local = runtime_node_id(graph, &reference.node_id);
        self.node(NodeSpec {
            id: local.clone(),
            control: reference.control,
            label: None,
            config: NodeConfig::Gain(GainConfig {
                channels: reference.channels,
                gain_db: reference.gain_db,
            }),
        })?;
        self.origin(local.clone(), graph, &reference.node_id, None);
        self.bind_control(native.clone(), local.clone());
        self.project_channels(
            endpoint(local, AUDIO_PORT_ID),
            endpoint(native, AUDIO_PORT_ID),
            reference.channels,
        )
    }

    fn source_reference(
        &mut self,
        graph: &str,
        reference: &SourceReference,
    ) -> Result<(), CompositionError> {
        let config = self
            .registry
            .sources
            .get(&reference.source_id)
            .ok_or_else(|| invalid("unknown shared capture source"))?;
        let (channels, _) = source_shape(config)?;
        if reference.channels == 0
            || reference.channels > channels
            || !reference.gain_db.is_finite()
        {
            return Err(invalid("invalid shared source reference channel/gain"));
        }
        let native = source_node_id(&reference.source_id);
        if !self.sources.contains_key(&reference.source_id) {
            self.node(NodeSpec {
                id: native.clone(),
                control: crate::NodeControl::default(),
                label: None,
                config: config.clone(),
            })?;
            self.sources
                .insert(reference.source_id.clone(), native.clone());
        }
        let local = runtime_node_id(graph, &reference.node_id);
        self.node(NodeSpec {
            id: local.clone(),
            control: reference.control,
            label: None,
            config: NodeConfig::Gain(GainConfig {
                channels: reference.channels,
                gain_db: reference.gain_db,
            }),
        })?;
        self.origin(local.clone(), graph, &reference.node_id, None);
        self.bind_control(native.clone(), local.clone());
        if let Some(id) = &reference.forwarding_endpoint {
            self.bind_device_controls(id, &local);
            if reference.control.enabled {
                self.playback_intents.push(ApplicationPlaybackIntent {
                    source_id: reference.source_id.clone(),
                    capture_node_id: native.clone(),
                    mask_node_id: local.clone(),
                    forwarding_endpoint: id.clone(),
                });
            }
        }
        self.project_channels(
            endpoint(native, AUDIO_PORT_ID),
            endpoint(local, AUDIO_PORT_ID),
            reference.channels,
        )
    }

    fn reference(
        &mut self,
        graph: &str,
        reference: &DeviceReference,
    ) -> Result<(), CompositionError> {
        crate::validate_virtual_endpoint_gain(reference.gain_db)
            .map_err(|error| invalid(error.to_string()))?;
        if (reference.write_channels.is_none() && reference.read.is_none())
            || !reference.gain_db.is_finite()
        {
            return Err(invalid(
                "device reference has no direction or has invalid gain",
            ));
        }
        let device = self
            .registry
            .get(&reference.device_id)
            .ok_or_else(|| invalid("unknown shared device"))?;
        if let Some(read) = reference.read {
            let width = device.capture.map_or(0, |stream| stream.channels);
            let source = endpoint(device_node_id(&device.id, "snapshot"), AUDIO_PORT_ID);
            if read.channels == 0 || read.channels > width {
                return Err(invalid("virtual read width exceeds canonical stream"));
            }
            let node = reference_node_id(graph, &reference.node_id, "read");
            self.node(NodeSpec {
                id: node.clone(),
                control: reference.control,
                label: None,
                config: NodeConfig::Gain(GainConfig {
                    channels: read.channels,
                    gain_db: reference.gain_db,
                }),
            })?;
            self.origin(
                node.clone(),
                graph,
                &reference.node_id,
                Some(FROM_OS_PORT_ID),
            );
            self.bind_device_controls(&reference.device_id, &node);
            self.project_channels(source, endpoint(node, AUDIO_PORT_ID), read.channels)?;
        }
        if let Some(channels) = reference.write_channels {
            self.write_reference(graph, reference, channels)?;
        }
        Ok(())
    }

    fn write_reference(
        &mut self,
        graph: &str,
        reference: &DeviceReference,
        channels: u16,
    ) -> Result<(), CompositionError> {
        let device = self
            .registry
            .get(&reference.device_id)
            .ok_or_else(|| invalid("unknown shared device"))?;
        let available = device
            .render
            .ok_or_else(|| invalid("shared device has no render stream"))?
            .channels;
        if channels == 0 || channels > available {
            return Err(invalid("virtual write width exceeds canonical stream"));
        }
        let node = reference_node_id(graph, &reference.node_id, "write");
        self.node(NodeSpec {
            id: node.clone(),
            control: reference.control,
            label: None,
            config: NodeConfig::Gain(GainConfig {
                channels,
                gain_db: 0.0,
            }),
        })?;
        self.origin(node.clone(), graph, &reference.node_id, Some(TO_OS_PORT_ID));
        self.bind_device_controls(&reference.device_id, &node);
        self.project_channels(
            endpoint(node, AUDIO_PORT_ID),
            endpoint(device_node_id(&device.id, "mix"), AUDIO_PORT_ID),
            channels,
        )?;
        Ok(())
    }

    fn map_endpoint(
        graph: &str,
        value: &AudioEndpoint,
        source: bool,
        nodes: &BTreeSet<NodeId>,
        references: &BTreeMap<NodeId, &DeviceReference>,
    ) -> Result<AudioEndpoint, CompositionError> {
        if !nodes.contains(&value.node_id) {
            return Err(invalid("edge references unknown local node"));
        }
        let Some(reference) = references.get(&value.node_id) else {
            return Ok(AudioEndpoint {
                node_id: runtime_node_id(graph, &value.node_id),
                port_id: value.port_id.clone(),
            });
        };
        let valid = if source {
            reference.read.is_some() && value.port_id.as_str() == FROM_OS_PORT_ID
        } else {
            reference.write_channels.is_some() && value.port_id.as_str() == TO_OS_PORT_ID
        };
        if !valid {
            return Err(invalid(
                "device reference edge uses unavailable direction/port",
            ));
        }
        Ok(endpoint(
            reference_node_id(graph, &value.node_id, if source { "read" } else { "write" }),
            AUDIO_PORT_ID,
        ))
    }

    fn origin(&mut self, runtime: NodeId, graph: &str, local: &NodeId, port: Option<&str>) {
        self.origins.insert(
            runtime,
            NodeOrigin {
                graph_id: graph.to_owned(),
                node_id: local.clone(),
                meter_port: port.map(PortId::from),
            },
        );
    }

    fn bind_control(&mut self, owner: NodeId, local: NodeId) {
        self.control_bindings.entry(owner).or_default().push(local);
    }

    fn bind_device_controls(&mut self, device: &VirtualEndpointId, local: &NodeId) {
        for side in ["native", "mix", "snapshot"] {
            let owner = device_node_id(device, side);
            if self.document.nodes.iter().any(|node| node.id == owner) {
                self.bind_control(owner, local.clone());
            }
        }
    }

    fn initialize_owner_controls(&mut self) {
        let controls: BTreeMap<_, _> = self
            .document
            .nodes
            .iter()
            .map(|node| (&node.id, node.control.enabled))
            .collect();
        let enabled_owners: BTreeMap<_, _> = self
            .control_bindings
            .iter()
            .map(|(owner, references)| {
                let enabled = references
                    .iter()
                    .any(|reference| controls.get(reference).copied().unwrap_or_default());
                (owner, enabled)
            })
            .collect();
        for node in &mut self.document.nodes {
            if let Some(&enabled) = enabled_owners.get(&node.id) {
                node.control = crate::NodeControl {
                    enabled,
                    muted: false,
                };
            }
        }
    }
    fn node(&mut self, node: NodeSpec) -> Result<(), CompositionError> {
        if self.document.nodes.len() >= self.context.limits.max_nodes {
            return Err(invalid("composed node budget exceeded"));
        }
        self.document.nodes.push(node);
        Ok(())
    }
    fn edge(&mut self, edge: EdgeSpec) -> Result<(), CompositionError> {
        if self.document.edges.len() >= self.context.limits.max_edges {
            return Err(invalid("composed edge budget exceeded"));
        }
        self.document.edges.push(edge);
        Ok(())
    }
    fn link(
        &mut self,
        source: AudioEndpoint,
        destination: AudioEndpoint,
    ) -> Result<(), CompositionError> {
        self.edge(EdgeSpec {
            id: EdgeId::new(format!("internal:{}", self.document.edges.len())),
            source,
            destination,
            channel_mapping: crate::ChannelMapping::default(),
        })
    }
    fn project_channels(
        &mut self,
        source: AudioEndpoint,
        destination: AudioEndpoint,
        channels: u16,
    ) -> Result<(), CompositionError> {
        self.edge(EdgeSpec {
            id: EdgeId::new(format!("internal:{}", self.document.edges.len())),
            source,
            destination,
            channel_mapping: crate::ChannelMapping::Explicit {
                routes: (0..channels)
                    .map(|channel| crate::ChannelRoute {
                        source_channel: channel,
                        destination_channel: channel,
                    })
                    .collect(),
            },
        })
    }
    pub(super) fn finish(
        mut self,
        compiler: &GraphCompiler,
    ) -> Result<ComposedGraph, CompositionError> {
        self.initialize_owner_controls();
        let execution = compiler
            .compile(&self.document, self.context)
            .map_err(CompositionError::Compile)?;
        Ok(ComposedGraph {
            document: self.document,
            execution,
            node_origins: self.origins,
            native_control_bindings: self.control_bindings,
            native_devices: self.devices,
            device_buses: self.buses,
            native_sources: self.sources,
            native_outputs: self.outputs,
            playback_intents: self.playback_intents,
        })
    }
}
