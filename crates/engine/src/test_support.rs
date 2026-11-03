//! Owned ports for portable controller policies; native worker contracts stay in adapters.
use crate::document_files::memory::MemoryDocuments;
use crate::processor_ports::{
    ProcessorCatalog, ProcessorControlError, ProcessorControls, ProcessorEditorCloseObservation,
    ProcessorLatencySnapshot, ProcessorReuseTransaction, ProcessorStateError,
    ProcessorStateSnapshot, ProcessorStates, ProcessorStorageError,
};
use crate::recording_ports::{OpenRecording, RecordingBackend, RecordingError, RecordingHold};
use crate::{
    Arc, BTreeMap, CompileContext, ContractOnlyVirtualEndpointBackend, DEFAULT_ENDPOINT_NODE_ID,
    DriverState, EngineGraphControllerConfig, ExternalProcessorCatalog, FixedRuntimeStatus, Mutex,
    NodeConfig, NodeId, NodeSpec, NoopGraphResourceBackend, PROTOCOL_VERSION, PeakMeterBank,
    PrepareProcessorError, ProcessorEditorStatus, ProcessorFactory, ProtocolError, RenderFormat,
    default_peak_meter_bank,
};
use mixrelay_audio_engine::{
    BlockProcessor, RecordingSink, RecordingSinkError, RecordingSinks, RenderPlan, RenderPlanError,
};
use mixrelay_audio_graph::{ExecutionPlan, GraphCompiler};
use mixrelay_audio_io::recording::{
    RecorderAction, RecorderRequest, RecorderState, RecorderStatus,
};
use mixrelay_processor_state::{StateBlobSink, StateBlobSource, StateError, StateRef};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Default)]
pub(crate) struct EmptyCatalog;
impl ProcessorCatalog for EmptyCatalog {
    fn ports(&self, _: &str) -> Option<mixrelay_audio_graph::ProviderPorts> {
        None
    }
    fn protocol_catalog(&self) -> ExternalProcessorCatalog {
        ExternalProcessorCatalog {
            protocol_version: PROTOCOL_VERSION,
            processors: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
    fn parameter_is_writable(&self, _: &str, _: u32) -> bool {
        false
    }
    fn rescan(&self, _: &[PathBuf]) -> Result<(), ProtocolError> {
        Ok(())
    }
}
#[derive(Default)]
pub(crate) struct EmptyControls;
struct Reuse;
impl ProcessorReuseTransaction for Reuse {
    fn commit(self: Box<Self>) {}
}
fn unavailable_control(node: &NodeId) -> ProcessorControlError {
    ProcessorControlError::diagnostic(format!("processor node is not active: {node}"))
}
impl ProcessorControls for EmptyControls {
    fn snapshot_state(
        &self,
        _: &NodeId,
        _: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError> {
        Err(ProcessorStateError::Unavailable)
    }
    fn reuse_transaction(&self) -> Box<dyn ProcessorReuseTransaction> {
        Box::new(Reuse)
    }
    fn invalidate_reuse(&self) {}
    fn has_worker(&self, _: &NodeId) -> bool {
        false
    }
    fn failed_worker_instance(&self, _: &NodeId) -> Option<u64> {
        None
    }
    fn begin_failed_worker_retry(
        &self,
        node: &NodeId,
        _: u64,
        _: Duration,
    ) -> Result<(), ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn set_parameter(&self, node: &NodeId, _: u32, _: f64) -> Result<(), ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn editor_status(&self, node: &NodeId) -> Result<ProcessorEditorStatus, ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn set_editor_open(
        &self,
        node: &NodeId,
        _: bool,
    ) -> Result<ProcessorEditorStatus, ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn cancel_editor_gesture(&self, node: &NodeId) -> Result<(), ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn editor_frame(
        &self,
        node: &NodeId,
    ) -> Result<mixrelay_control_protocol::ProcessorEditorFrame, ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn editor_input(
        &self,
        node: &NodeId,
        _: &mixrelay_control_protocol::ProcessorEditorInput,
    ) -> Result<(), ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn editor_close_observation(
        &self,
        node: &NodeId,
    ) -> Result<ProcessorEditorCloseObservation, ProcessorControlError> {
        Err(unavailable_control(node))
    }
    fn snapshot_state_for_editor_close(
        &self,
        _: &NodeId,
        _: ProcessorEditorCloseObservation,
        _: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError> {
        Err(ProcessorStateError::Unavailable)
    }
    fn processing_time_ns(&self, _: &NodeId) -> Option<u64> {
        None
    }
    fn latency_snapshot(&self, _: &NodeId) -> Option<ProcessorLatencySnapshot> {
        None
    }
}

#[derive(Default)]
pub(crate) struct MemoryAssets(Mutex<BTreeMap<StateRef, Vec<u8>>>);
impl StateBlobSource for MemoryAssets {
    fn verify(&self, reference: &StateRef) -> Result<(), StateError> {
        self.0
            .lock()
            .get(reference)
            .map(|_| ())
            .ok_or(StateError::MissingBlob)
    }
    fn copy_verified(
        &self,
        reference: &StateRef,
        output: &mut dyn Write,
    ) -> Result<(), StateError> {
        let bytes = self.0.lock();
        output.write_all(bytes.get(reference).ok_or(StateError::MissingBlob)?)?;
        Ok(())
    }
}
impl StateBlobSink for MemoryAssets {
    fn store_verified(
        &self,
        reader: &mut dyn Read,
        reference: &StateRef,
    ) -> Result<(), StateError> {
        let mut bytes = Vec::new();
        reader
            .take(reference.byte_length())
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != reference.byte_length()
            || format!("{:x}", Sha256::digest(&bytes)) != reference.sha256()
        {
            return Err(StateError::Integrity);
        }
        self.0.lock().insert(reference.clone(), bytes);
        Ok(())
    }
}
#[derive(Default)]
pub(crate) struct MemoryStates {
    selected: Mutex<Option<Arc<MemoryAssets>>>,
    namespaces: Mutex<BTreeMap<PathBuf, Arc<MemoryAssets>>>,
}
impl ProcessorStates for MemoryStates {
    fn select_namespace(&self, path: &Path) {
        let selected = self
            .namespaces
            .lock()
            .entry(path.to_owned())
            .or_default()
            .clone();
        *self.selected.lock() = Some(selected);
    }
    fn snapshot(&self) -> Result<ProcessorStateSnapshot, ProcessorStorageError> {
        self.selected
            .lock()
            .clone()
            .map(|assets| ProcessorStateSnapshot::new(assets))
            .ok_or(ProcessorStorageError::Unconfigured)
    }
}

#[derive(Default)]
pub(crate) struct Recordings(Mutex<BTreeMap<String, RecorderStatus>>);
struct Held;
impl RecordingHold for Held {}
impl RecordingBackend for Recordings {
    fn status(&self, id: &str) -> RecorderStatus {
        self.0.lock().get(id).cloned().unwrap_or_default()
    }
    fn open_take_taps(&self) -> Vec<OpenRecording> {
        self.0
            .lock()
            .iter()
            .filter(|(_, status)| {
                matches!(
                    status.state,
                    RecorderState::Recording | RecorderState::Paused
                )
            })
            .map(|(id, status)| OpenRecording {
                id: id.clone(),
                state: status.state,
                hold: Box::new(Held),
            })
            .collect()
    }
    fn command(
        &self,
        id: &str,
        request: RecorderRequest,
    ) -> Result<RecorderStatus, RecordingError> {
        let mut recordings = self.0.lock();
        let state = recordings.entry(id.to_owned()).or_default();
        state.state = match request.action {
            RecorderAction::Record => RecorderState::Recording,
            RecorderAction::Pause => RecorderState::Paused,
            RecorderAction::Stop => RecorderState::Stopped,
        };
        if request.path.is_some() {
            state.path = request.path;
        }
        if request.sample_rate.is_some() {
            state.sample_rate = request.sample_rate;
        }
        if request.encoding.is_some() {
            state.encoding = request.encoding;
        }
        Ok(state.clone())
    }
    fn stop_all(&self) -> Result<(), RecordingError> {
        self.0
            .lock()
            .values_mut()
            .for_each(|status| status.state = RecorderState::Stopped);
        Ok(())
    }
    fn reap_unused(&self) -> Result<(), RecordingError> {
        Ok(())
    }
}

// The portable fixture admits saved topology without preparing a native plug-in.
struct SavedPorts;
impl mixrelay_audio_graph::NodeResolver for SavedPorts {
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<mixrelay_audio_graph::ResolvedNode, mixrelay_audio_graph::NodeResolutionError> {
        use mixrelay_audio_graph::{
            BuiltInNodeResolver, CompiledPort, NodeResolutionError, PortDirection, ResolvedNode,
        };
        let NodeConfig::Provider(config) = &node.config else {
            return BuiltInNodeResolver.resolve(node, context);
        };
        let ports =
            config
                .ports
                .as_ref()
                .ok_or_else(|| NodeResolutionError::CapabilityUnavailable {
                    reason: format!("processor is not installed: {}", config.type_id),
                })?;
        ports
            .validate()
            .map_err(|error| NodeResolutionError::InvalidConfiguration {
                reason: error.to_string(),
            })?;
        if ports
            .inputs
            .iter()
            .chain(&ports.outputs)
            .any(|bus| bus.channels > context.limits.max_channels_per_port)
        {
            return Err(NodeResolutionError::InvalidConfiguration {
                reason: "processor bus width exceeds graph limits".to_owned(),
            });
        }
        Ok(ResolvedNode {
            inputs: ports
                .inputs
                .iter()
                .map(|bus| CompiledPort::new(bus.id.clone(), PortDirection::Input, bus.channels))
                .collect(),
            outputs: ports
                .outputs
                .iter()
                .map(|bus| CompiledPort::new(bus.id.clone(), PortDirection::Output, bus.channels))
                .collect(),
        })
    }
}

pub(crate) struct DspFactory {
    pub(crate) meters: Arc<PeakMeterBank>,
    pub(crate) render_prepare: fn(&ExecutionPlan) -> Result<RenderPlan, RenderPlanError>,
}
impl DspFactory {
    pub(crate) fn new(meters: Arc<PeakMeterBank>) -> Self {
        Self {
            meters,
            render_prepare: RenderPlan::prepare,
        }
    }
}
struct SilentSinks;
struct SilentSink;
impl RecordingSink for SilentSink {
    fn capture(&self, _: &[f32], _: u32) {}
}
impl RecordingSinks for SilentSinks {
    fn prepare(&self, _: &str, _: u16) -> Result<Box<dyn RecordingSink>, RecordingSinkError> {
        Ok(Box::new(SilentSink))
    }
}
impl ProcessorFactory for DspFactory {
    fn compiler(&self) -> GraphCompiler {
        GraphCompiler::new(SavedPorts)
    }
    fn prepare(
        &self,
        execution: &ExecutionPlan,
    ) -> Result<Box<BlockProcessor>, PrepareProcessorError> {
        let render = (self.render_prepare)(execution)?;
        let mut processor =
            BlockProcessor::with_peak_meters(Arc::new(render), self.meters.clone())?;
        processor
            .attach_recorders(&SilentSinks)
            .map_err(PrepareProcessorError::Recorder)?;
        Ok(Box::new(processor))
    }
}

pub(crate) fn configuration(format: RenderFormat) -> EngineGraphControllerConfig {
    let meters = default_peak_meter_bank();
    EngineGraphControllerConfig {
        clock: Arc::new(mixrelay_time::ManualClock::default()),
        format,
        meters: meters.clone(),
        initial_bindings: Vec::new(),
        endpoint_backend: Arc::new(ContractOnlyVirtualEndpointBackend::default()),
        required_endpoint_node_id: Some(DEFAULT_ENDPOINT_NODE_ID.into()),
        processor_catalog: Arc::new(EmptyCatalog),
        processor_controls: Arc::new(EmptyControls),
        processor_files: Arc::new(MemoryStates::default()),
        recording: Arc::new(Recordings::default()),
        factory: Arc::new(DspFactory::new(meters)),
        documents: Arc::new(MemoryDocuments::default()),
        incarnation: "fixture-engine".into(),
        graph_resources: Arc::new(NoopGraphResourceBackend),
        runtime_status: Arc::new(FixedRuntimeStatus::new(DriverState::Ready)),
    }
}
#[cfg(test)]
pub(crate) fn controller(
    exchange: Arc<ProcessorExchange>,
    node: impl Into<NodeId>,
    format: RenderFormat,

    clock: Arc<dyn session::ProcessingTime>,
) -> EngineGraphController {
    EngineGraphController::new_with_configuration(
        exchange,
        EngineGraphControllerConfig {
            clock,
            required_endpoint_node_id: Some(node.into()),

            ..configuration(format)
        },
    )
}

#[cfg(test)]
pub(crate) fn read_document(
    files: &dyn document_files::DocumentFiles,
    path: &Path,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    files.open(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
pub(crate) struct Documents {
    pub(crate) files: MemoryDocuments,
    path: PathBuf,
}
#[cfg(test)]
impl Documents {
    pub(crate) fn new(label: &str) -> Self {
        Self {
            files: MemoryDocuments::default(),
            path: Path::new("/engine-tests").join(label),
        }
    }
    pub(crate) fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }
    pub(crate) fn bind(&self, mut controller: EngineGraphController) -> EngineGraphController {
        controller.documents = Arc::new(self.files.clone());
        controller.power =
            crate::execution_choice::ExecutionChoice::new(controller.documents.clone());
        controller
    }
    pub(crate) fn catalog(&self, controller: EngineGraphController) -> EngineGraphController {
        self.bind(controller)
            .with_graph_catalog_path(self.join("graphs.json"))
    }
}

#[cfg(test)]
impl EngineGraphController {
    pub(crate) fn with_test_documents(mut self, files: &MemoryDocuments) -> Self {
        self.documents = Arc::new(files.clone());
        self.power = crate::execution_choice::ExecutionChoice::new(self.documents.clone());
        self
    }
}

#[cfg(test)]
use crate::{EngineGraphController, ProcessorExchange, document_files, session};
