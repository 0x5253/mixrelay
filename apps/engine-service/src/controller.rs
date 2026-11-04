//! The engine controller as the control server sees it: the domain controller composed over
//! the recorder files, processor host and state files that this native service owns.
mod control_api;

use crate::adapters::{
    processor_factory::BuiltInProcessorFactory,
    processor_ports::{HostCatalog, HostControls, HostStateFiles},
    recording_backend::RegistryRecordingBackend,
};
use mixrelay_audio_graph::{GraphDocument, NodeId, RenderFormat};
use mixrelay_audio_io::VirtualEndpointBackend;
use mixrelay_audio_recorder::RecorderRegistry;
use mixrelay_control_protocol::{GraphApplyResponse, ProtocolError};
use mixrelay_engine_core::{GraphResourceBackend, RuntimeBackend};
use mixrelay_engine_host::{
    control_maintenance::{ControlMaintenance, MaintenanceTarget},
    persistence::config_directory,
};
use mixrelay_processor_host::{
    ExternalProcessorFactory, ProcessorCatalog, ProcessorControlRegistry, ProcessorFiles,
};
use mixrelay_render::{MeterBinding, PeakMeterBank, ProcessorExchange, RenderPlan};
use std::sync::Arc;

/// The platform owners each service supplies; the controller adds the owners every
/// service shares.
pub struct EngineGraphControllerConfig {
    /// Sampled by hosted audio processing; `now` must not allocate or block.
    pub clock: Arc<dyn mixrelay_engine_core::session::ProcessingTime>,
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
    core: mixrelay_engine_core::EngineGraphController,
}

impl EngineGraphController {
    pub fn new_with_initial_document(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
        document: GraphDocument,
    ) -> Self {
        Self {
            core: mixrelay_engine_core::EngineGraphController::new_with_initial_document(
                exchange,
                compose_native(config),
                document,
            ),
        }
    }

    pub fn new_with_configuration(
        exchange: Arc<ProcessorExchange>,
        config: EngineGraphControllerConfig,
    ) -> Self {
        Self {
            core: mixrelay_engine_core::EngineGraphController::new_with_configuration(
                exchange,
                compose_native(config),
            ),
        }
    }

    #[must_use]
    pub fn after_repeated_failures(mut self, tripped: bool) -> Self {
        self.core = self.core.after_repeated_failures(tripped);
        self
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
    /// Reports a rejected native format or graph publication.
    pub fn reconcile_observed_device_rate(
        &self,
    ) -> Result<Option<GraphApplyResponse>, ProtocolError> {
        self.core.reconcile_observed_device_rate()
    }

    /// Exports the current native state before a recovery qualification snapshot.
    /// # Errors
    /// Reports capture, revision or persistence failure.
    pub fn checkpoint_processor_state(&self, node: Option<&str>) -> Result<(), ProtocolError> {
        self.core.checkpoint_processor_state(node)
    }

    /// # Errors
    /// Reports failure to start the joined maintenance worker.
    pub fn start_control_maintenance(self: &Arc<Self>) -> Result<ControlMaintenance, String> {
        ControlMaintenance::start(self.clone())
    }
}

impl MaintenanceTarget for EngineGraphController {
    fn maintain_control(&self, ledger: &mut mixrelay_engine_core::maintenance::CloseLedger) {
        self.core.maintain_control(ledger);
    }

    fn finalize_control(&self) -> Result<(), ProtocolError> {
        self.core.finalize_control()
    }
}

/// Composes the domain configuration over the threaded recorder files and processor state
/// files that this native service owns.
fn compose_native(
    config: EngineGraphControllerConfig,
) -> mixrelay_engine_core::EngineGraphControllerConfig {
    compose(
        config,
        RecorderRegistry::threaded_files(),
        ProcessorFiles::default(),
    )
}

/// Every engine port that reaches recorder or processor files shares the same `recorders`
/// and `processor_files`, so a take or a saved state is seen identically by preparation,
/// control and persistence.
fn compose(
    config: EngineGraphControllerConfig,
    recorders: RecorderRegistry,
    processor_files: ProcessorFiles,
) -> mixrelay_engine_core::EngineGraphControllerConfig {
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
    let processor_controls = ProcessorControlRegistry::new(Arc::new(processor_files.clone()));
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
    mixrelay_engine_core::EngineGraphControllerConfig {
        clock,
        format,
        meters,
        initial_bindings,
        endpoint_backend,
        required_endpoint_node_id,
        graph_resources,
        runtime_status,
        processor_catalog: Arc::new(HostCatalog(processor_catalog)),
        processor_controls: Arc::new(HostControls(processor_controls)),
        processor_files: Arc::new(HostStateFiles(processor_files)),
        recording: Arc::new(RegistryRecordingBackend(recorders)),
        factory,
        documents: Arc::new(mixrelay_engine_host::document_files::DiskDocuments),
        incarnation: mixrelay_engine_host::identity::new_incarnation(),
    }
}

#[cfg(test)]
mod tests;
