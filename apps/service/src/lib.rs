//! Composes engine domain ports with native processor, recorder and file adapters.
mod control_api;
pub mod control_service;
mod processor_bridge;
mod processor_preparation;
mod recording_bridge;
mod recording_sinks;
// Native lifetime owners remain outside the engine domain.
#[cfg(test)]
mod session;
use mixrelay_audio_engine::{
    BlockProcessor, MeterBinding, PeakMeterBank, ProcessorExchange, RenderPlan,
};
#[cfg(test)]
use mixrelay_audio_graph::composition as graph_composition;
#[cfg(test)]
use mixrelay_audio_graph::{
    ChannelLayout, NodeSpec, StreamSpec, VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_audio_graph::{
    ExecutionPlan, GraphCompiler, GraphDocument, NodeConfig, NodeId, RenderFormat,
};
use mixrelay_audio_io::VirtualEndpointBackend;
#[cfg(test)]
use mixrelay_audio_io::{HostAudioBackendError, VirtualEndpointCapabilities};
#[cfg(test)]
use mixrelay_control_protocol::ErrorCode;
use mixrelay_control_protocol::{
    AppStatus, ExternalProcessorCatalog, GraphApplyRequest, GraphApplyResponse, GraphRuntimeStatus,
    MeterSnapshot, ProcessorEditorStatus, ProcessorParameterUpdateResponse, ProcessorRetryRequest,
    ProcessorScanRequest, ProtocolError, SessionReconfigureRequest,
};
use mixrelay_control_server::ControlBackend;
use mixrelay_engine::{GraphResourceBackend, RuntimeBackend};
use mixrelay_engine::{PrepareProcessorError, ProcessorFactory};
#[cfg(test)]
use mixrelay_engine::{
    ProcessingControlBackend, RuntimeStatusBackend, SessionFormatBackend, default_duplex_document,
};
use mixrelay_engine_runtime::persistence::config_directory;
use mixrelay_processor_host::{
    CatalogNodeResolver, ExternalProcessorFactory, ProcessorCatalog, ProcessorControlRegistry,
    ProcessorFiles,
};
use processor_preparation::BuiltInProcessorFactory;
use std::sync::Arc;
pub struct EngineGraphControllerConfig {
    /// Sampled by hosted audio processing; `now` must not allocate or block.
    pub clock: Arc<dyn mixrelay_engine::session::ProcessingTime>,
    pub format: RenderFormat,
    pub meters: Arc<PeakMeterBank>,
    pub initial_bindings: Vec<MeterBinding>,
    pub endpoint_backend: Arc<dyn VirtualEndpointBackend>,
    pub required_endpoint_node_id: Option<NodeId>,
    pub processor_catalog: ProcessorCatalog,
    pub graph_resources: Arc<dyn GraphResourceBackend>,
    pub runtime_status: Arc<dyn RuntimeBackend>,
}

/// Native composition owner implementing the control-server's adapter contract.
pub struct EngineGraphController {
    core: mixrelay_engine::EngineGraphController,
    #[cfg(test)]
    recording_registry: mixrelay_audio_recorder::RecorderRegistry,
    #[cfg(test)]
    processor_files: ProcessorFiles,
    #[cfg(test)]
    exchange: Arc<ProcessorExchange>,
    #[cfg(test)]
    incarnation: String,
}
impl EngineGraphController {
    #[must_use]
    pub fn after_repeated_failures(mut self, tripped: bool) -> Self {
        self.core = self.core.after_repeated_failures(tripped);
        self
    }
    /// # Errors
    /// Reports a rejected native format or graph publication.
    pub fn reconcile_observed_device_rate(
        &self,
    ) -> Result<Option<GraphApplyResponse>, ProtocolError> {
        self.core.reconcile_observed_device_rate()
    }
    pub fn new_with_initial_document(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
        document: GraphDocument,
    ) -> Self {
        let (config, recording_registry, processor_files) = compose(config);
        #[cfg(test)]
        let incarnation = config.incarnation.clone();
        #[cfg(test)]
        let test_exchange = exchange.clone();
        let core = mixrelay_engine::EngineGraphController::new_with_initial_document(
            exchange, config, document,
        );
        #[cfg(not(test))]
        drop((recording_registry, processor_files));
        Self {
            core,
            #[cfg(test)]
            recording_registry,
            #[cfg(test)]
            processor_files,
            #[cfg(test)]
            exchange: test_exchange,
            #[cfg(test)]
            incarnation,
        }
    }
    pub fn new_with_configuration(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
    ) -> Self {
        let (config, recording_registry, processor_files) = compose(config);
        #[cfg(test)]
        let incarnation = config.incarnation.clone();
        #[cfg(test)]
        let test_exchange = exchange.clone();
        let core = mixrelay_engine::EngineGraphController::new_with_configuration(exchange, config);
        #[cfg(not(test))]
        drop((recording_registry, processor_files));
        Self {
            core,
            #[cfg(test)]
            recording_registry,
            #[cfg(test)]
            processor_files,
            #[cfg(test)]
            exchange: test_exchange,
            #[cfg(test)]
            incarnation,
        }
    }
    /// Exports the current native state before a recovery qualification snapshot.
    /// # Errors
    /// Reports capture, revision or persistence failure.
    pub fn checkpoint_processor_state(&self, node: Option<&str>) -> Result<(), ProtocolError> {
        self.core.checkpoint_processor_state(node)
    }
    #[must_use]
    pub fn with_graph_catalog_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.core = self.core.with_graph_catalog_path(path.into());
        self
    }
    #[must_use]
    pub fn with_graph_catalog(mut self) -> Self {
        match config_directory() {
            Ok(directory) => self.with_graph_catalog_path(directory.join("graphs.json")),
            Err(error) => {
                self.core = self.core.with_unavailable_graph_catalog(&error);
                self
            }
        }
    }
    /// # Errors
    /// Reports failure to start the joined maintenance worker.
    pub fn start_control_maintenance(
        self: &Arc<Self>,
    ) -> Result<mixrelay_engine_runtime::control_maintenance::ControlMaintenance, String> {
        mixrelay_engine_runtime::control_maintenance::ControlMaintenance::start(self.clone())
    }
}
impl mixrelay_engine_runtime::control_maintenance::MaintenanceTarget for EngineGraphController {
    fn maintain_control(&self, ledger: &mut mixrelay_engine::maintenance::CloseLedger) {
        self.core.maintain_control(ledger);
    }
    fn finalize_control(&self) -> Result<(), ProtocolError> {
        self.core.finalize_control()
    }
}
fn compose(
    config: EngineGraphControllerConfig,
) -> (
    mixrelay_engine::EngineGraphControllerConfig,
    mixrelay_audio_recorder::RecorderRegistry,
    ProcessorFiles,
) {
    let EngineGraphControllerConfig {
        clock,
        format,
        meters,
        initial_bindings,
        endpoint_backend,
        required_endpoint_node_id,
        processor_catalog,
        graph_resources,
        runtime_status,
    } = config;
    let processor_files = ProcessorFiles::default();
    let processor_controls = ProcessorControlRegistry::new(Arc::new(processor_files.clone()));
    let recorders = mixrelay_audio_recorder::RecorderRegistry::threaded_files();
    let factory = Arc::new(BuiltInProcessorFactory {
        recorders: recorders.clone(),
        catalog: processor_catalog.clone(),
        hosted: ExternalProcessorFactory::with_controls(
            processor_catalog.clone(),
            processor_controls.clone(),
            processor_files.clone(),
            clock.clone(),
        ),
        meters: meters.clone(),
        render_prepare: RenderPlan::prepare,
    });
    (
        mixrelay_engine::EngineGraphControllerConfig {
            clock,
            format,
            meters,
            initial_bindings,
            endpoint_backend,
            required_endpoint_node_id,
            graph_resources,
            runtime_status,
            processor_catalog: Arc::new(processor_bridge::HostCatalog(processor_catalog)),
            processor_controls: Arc::new(processor_bridge::HostControls(processor_controls)),
            processor_files: Arc::new(processor_bridge::HostStateFiles(processor_files.clone())),
            recording: Arc::new(recording_bridge::RegistryRecordingBackend(
                recorders.clone(),
            )),
            factory,
            documents: Arc::new(mixrelay_engine_runtime::document_files::DiskDocuments),
            incarnation: mixrelay_engine_runtime::identity::new_incarnation(),
        },
        recorders,
        processor_files,
    )
}

#[cfg(test)]
mod controller_identity_native_tests;
#[cfg(test)]
mod controller_test_support;
#[cfg(test)]
mod endpoint_settings_tests;
#[cfg(test)]
mod recorder_policy_native_tests;
#[cfg(test)]
use mixrelay_engine_runtime::identity::new_incarnation;

#[cfg(test)]
mod workspace_native_tests;

#[cfg(test)]
mod controller_native_tests;

#[cfg(test)]
mod catalog_recording_native_tests;

#[cfg(test)]
use mixrelay_engine::{NoopGraphResourceBackend, default_peak_meter_bank};
