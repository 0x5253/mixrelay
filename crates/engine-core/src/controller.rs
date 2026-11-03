
pub struct EngineGraphController {
    clock: Arc<dyn session::ProcessingTime>,
    power: execution_choice::ExecutionChoice,
    /// Random identity of this engine run; telemetry from a replaced engine never mixes in.
    incarnation: String,
    node_control_warning: Mutex<Option<String>>,
    recorders: recorder_sessions::RecorderSessions,
    exchange: Arc<ProcessorExchange>,
    factory: Arc<dyn ProcessorFactory>,
    graph: RwLock<GraphDocument>,
    context: RwLock<CompileContext>,
    mutation: Mutex<()>,
    catalog: graph_catalog::CatalogStorage,
    catalog_restore: graph_catalog::CatalogRecall,
    required_endpoint_node_id: Option<NodeId>,
    endpoint_backend: Arc<dyn VirtualEndpointBackend>,
    meter_bindings: RwLock<BTreeMap<u64, Vec<MeterBinding>>>,
    execution_views: RwLock<BTreeMap<u64, execution_control::ExecutionView>>,
    control_snapshot: RwLock<Option<node_control_snapshot::ControlReadView>>,
    meters: Arc<PeakMeterBank>,
    meter_publisher: Mutex<meter_snapshot::MeterPublisher>,
    telemetry_status: Mutex<Option<control_telemetry::CachedStatus>>,
    processor_catalog: Arc<dyn ProcessorCatalog>,
    processor_controls: Arc<dyn ProcessorControls>,
    processor_files: Arc<dyn ProcessorStates>,
    graph_resources: Arc<dyn GraphResourceBackend>,
    runtime_status: Arc<dyn RuntimeBackend>,
    documents: Arc<dyn document_files::DocumentFiles>,
}

pub struct EngineGraphControllerConfig {
    /// Sampled by hosted audio processing; `now` must not allocate or block.
    pub clock: Arc<dyn session::ProcessingTime>,
    pub format: RenderFormat,
    pub meters: Arc<PeakMeterBank>,
    pub initial_bindings: Vec<MeterBinding>,
    pub endpoint_backend: Arc<dyn VirtualEndpointBackend>,
    pub required_endpoint_node_id: Option<NodeId>,
    pub processor_catalog: Arc<dyn ProcessorCatalog>,
    pub processor_controls: Arc<dyn ProcessorControls>,
    pub processor_files: Arc<dyn ProcessorStates>,
    pub recording: Arc<dyn recording_ports::RecordingBackend>,
    pub factory: Arc<dyn ProcessorFactory>,
    pub documents: Arc<dyn document_files::DocumentFiles>,
    pub incarnation: String,
    pub graph_resources: Arc<dyn GraphResourceBackend>,
    pub runtime_status: Arc<dyn RuntimeBackend>,
}

impl EngineGraphController {
    #[must_use]
    /// Publishes the document already compiled by a native adapter at startup.
    /// Capability flags do not imply that a default device node actually exists.
    ///
    /// # Panics
    /// Panics if the supplied document is not the active processor revision.
    pub fn new_with_initial_document(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
        document: GraphDocument,
    ) -> Self {
        assert_eq!(
            document.revision,
            exchange.revisions().active,
            "initial graph must match the active processor revision"
        );
        let EngineGraphControllerConfig {
            clock,
            format,
            meters,
            initial_bindings,
            endpoint_backend,
            required_endpoint_node_id,
            processor_catalog,
            processor_controls,
            processor_files,
            recording,
            factory,
            documents,
            incarnation,
            graph_resources,
            runtime_status,
        } = config;
        let active_revision = exchange.revisions().active;
        let mut meter_bindings = BTreeMap::new();
        meter_bindings.insert(active_revision, initial_bindings);
        let controller = Self {
            clock: Arc::clone(&clock),
            node_control_warning: Mutex::new(None),
            power: execution_choice::ExecutionChoice::new(Arc::clone(&documents)),
            incarnation,
            recorders: recorder_sessions::RecorderSessions::new(recording),
            exchange,
            factory,
            graph: RwLock::new(document),
            context: RwLock::new(CompileContext {
                format,
                limits: CompileLimits::default(),
            }),
            mutation: Mutex::new(()),
            catalog: graph_catalog::CatalogStorage::default(),
            catalog_restore: graph_catalog::CatalogRecall::default(),
            required_endpoint_node_id,
            endpoint_backend,
            meter_bindings: RwLock::new(meter_bindings),
            execution_views: RwLock::new(BTreeMap::new()),
            control_snapshot: RwLock::new(None),
            telemetry_status: Mutex::new(None),
            meter_publisher: Mutex::new(meter_snapshot::MeterPublisher::new(Arc::clone(&meters), clock.now())),
            meters,
            processor_catalog,
            processor_controls,
            processor_files,
            graph_resources,
            runtime_status,
            documents,
        };
        controller.refresh_control_snapshot();
        controller
    }

    /// Constructs a controller with the built-in session document selected from
    /// the configured capabilities. Native startup with an already prepared graph
    /// must use `new_with_initial_document` instead.
    #[must_use]
    pub fn new_with_configuration(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
    ) -> Self {
        let revision = exchange.revisions().active;
        let document = if config
            .runtime_status
            .status(config.format)
            .audio_session
            .is_some_and(|session| !session.capabilities.virtual_endpoint)
        {
            GraphDocument::new(revision, vec![], vec![])
        } else {
            default_session_document(revision, config.format)
        };
        Self::new_with_initial_document(exchange, config, document)
    }

    #[cfg(any(test, feature = "test-util"))]
    fn prepare_and_validate(
        &self,
        document: &GraphDocument,
        context: CompileContext,
    ) -> Result<execution_control::PreparedExecution, ProtocolError> {
        let catalog = self.catalog_candidate(document);
        let prepared = self.prepare_execution(document, catalog.as_ref(), context)?;
        self.execution_views
            .write()
            .insert(document.revision, prepared.view.clone());
        Ok(prepared)
    }

    fn validate_required_endpoint(&self, document: &GraphDocument) -> Result<(), ProtocolError> {
        if let Some(required) = &self.required_endpoint_node_id
            && !document.nodes.iter().any(|node| {
                node.id == *required && matches!(node.config, NodeConfig::VirtualEndpoint(_))
            })
        {
            return Err(ProtocolError {
                code: ErrorCode::GraphBindingUnsupported,
                parameters: serde_json::json!({
                    "requiredEndpointNodeId": required.as_str(),
                }),
            });
        }
        Ok(())
    }

    fn validate_session_bindings(
        &self,
        document: &GraphDocument,
        format: RenderFormat,
    ) -> Result<(), ProtocolError> {
        let session = self.runtime_status.capabilities_for_format(format);
        for node in &document.nodes {
            if !node.control.enabled {
                continue;
            }
            let reason = match &node.config {
                NodeConfig::ApplicationCapture(_)
                    if session.as_ref().is_some_and(|s| !s.application_capture) =>
                {
                    Some(
                        "application capture is not supported by the active adapter at this session format",
                    )
                }
                NodeConfig::VirtualEndpoint(endpoint)
                    if self
                        .endpoint_backend
                        .sample_rate(&endpoint.id)
                        .is_ok_and(|rate| endpoint.default_sample_rate != rate) =>
                {
                    Some("endpoint sample rate differs from its available native clock")
                }
                NodeConfig::AudioDeviceInput(_)
                    if session.as_ref().is_some_and(|s| !s.hardware_input) =>
                {
                    Some("hardware input is not implemented by this audio adapter")
                }
                NodeConfig::AudioDeviceOutput(_)
                    if session.as_ref().is_some_and(|s| !s.hardware_output) =>
                {
                    Some("hardware output is not implemented by this audio adapter")
                }
                NodeConfig::VirtualEndpoint(_)
                    if session.as_ref().is_some_and(|s| !s.virtual_endpoint) =>
                {
                    Some("the virtual endpoint driver is unavailable on this audio adapter")
                }
                _ => None,
            };
            if let Some(reason) = reason {
                return Err(ProtocolError {
                    code: ErrorCode::GraphBindingUnsupported,
                    parameters: serde_json::json!({ "nodeId": node.id.as_str(), "reason": reason,
                        "sampleRate": format.sample_rate, "blockFrames": format.block_frames }),
                });
            }
        }
        Ok(())
    }
}
