//! Connects engine processor ports to the native host and file-store owners.

use mixrelay_audio_graph::{NodeId, ProviderPorts};
use mixrelay_control_protocol::{
    ErrorCode, ExternalProcessorCatalog, ProcessorEditorFrame, ProcessorEditorInput,
    ProcessorEditorStatus, ProtocolError,
};
use mixrelay_engine_core::processor_ports::{
    self as ports, ProcessorControlError, ProcessorStateError,
};
use mixrelay_processor_host::{ProcessorCatalog, ProcessorControlRegistry, ProcessorFiles};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct HostCatalog(pub(crate) ProcessorCatalog);
impl ports::ProcessorCatalog for HostCatalog {
    fn ports(&self, type_id: &str) -> Option<ProviderPorts> {
        self.0.ports(type_id)
    }
    fn protocol_catalog(&self) -> ExternalProcessorCatalog {
        self.0.protocol_catalog()
    }
    fn parameter_is_writable(&self, type_id: &str, id: u32) -> bool {
        self.0.parameter_is_writable(type_id, id)
    }
    fn rescan(&self, paths: &[PathBuf]) -> Result<(), ProtocolError> {
        if let Some(path) = paths.iter().find(|path| !path.is_dir()) {
            return Err(ProtocolError {
                code: ErrorCode::ProcessorUnavailable,
                parameters: serde_json::json!({ "reason": "processor scan path is not a directory", "path": path }),
            });
        }
        self.0.rescan(paths);
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct HostControls(pub(crate) ProcessorControlRegistry);
struct HostReuse(mixrelay_processor_host::ProcessorReuseTransaction);
impl ports::ProcessorReuseTransaction for HostReuse {
    fn commit(self: Box<Self>) {
        self.0.commit();
    }
}
impl ports::ProcessorControls for HostControls {
    fn snapshot_state(
        &self,
        node: &NodeId,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError> {
        self.0.snapshot_state(node, timeout).map_err(state_error)
    }
    fn reuse_transaction(&self) -> Box<dyn ports::ProcessorReuseTransaction> {
        Box::new(HostReuse(self.0.reuse_transaction()))
    }
    fn invalidate_reuse(&self) {
        self.0.invalidate_reuse();
    }
    fn has_worker(&self, node: &NodeId) -> bool {
        self.0.has_worker(node)
    }
    fn failed_worker_instance(&self, node: &NodeId) -> Option<u64> {
        self.0.failed_worker_instance(node)
    }
    fn begin_failed_worker_retry(
        &self,
        node: &NodeId,
        expected: u64,
        cooldown: Duration,
    ) -> Result<(), ProcessorControlError> {
        self.0
            .begin_failed_worker_retry(node, expected, cooldown)
            .map_err(|diagnostic| ProcessorControlError::diagnostic(diagnostic.to_owned()))
    }
    fn set_parameter(
        &self,
        node: &NodeId,
        id: u32,
        value: f64,
    ) -> Result<(), ProcessorControlError> {
        self.0
            .set_parameter(node, id, value)
            .map_err(ProcessorControlError::diagnostic)
    }
    fn editor_status(&self, node: &NodeId) -> Result<ProcessorEditorStatus, ProcessorControlError> {
        self.0
            .editor_status(node)
            .map_err(ProcessorControlError::diagnostic)
    }
    fn set_editor_open(
        &self,
        node: &NodeId,
        open: bool,
    ) -> Result<ProcessorEditorStatus, ProcessorControlError> {
        self.0
            .set_editor_open(node, open)
            .map_err(ProcessorControlError::diagnostic)
    }
    fn cancel_editor_gesture(&self, node: &NodeId) -> Result<(), ProcessorControlError> {
        self.0
            .cancel_editor_gesture(node)
            .map_err(ProcessorControlError::diagnostic)
    }
    fn editor_frame(&self, node: &NodeId) -> Result<ProcessorEditorFrame, ProcessorControlError> {
        self.0
            .editor_frame(node)
            .map_err(ProcessorControlError::diagnostic)
    }
    fn editor_input(
        &self,
        node: &NodeId,
        input: &ProcessorEditorInput,
    ) -> Result<(), ProcessorControlError> {
        self.0
            .editor_input(node, input)
            .map_err(ProcessorControlError::diagnostic)
    }
    fn editor_close_observation(
        &self,
        node: &NodeId,
    ) -> Result<ports::ProcessorEditorCloseObservation, ProcessorControlError> {
        self.0
            .editor_close_observation(node)
            .map(|value| ports::ProcessorEditorCloseObservation {
                instance_id: value.instance_id,
                close_generation: value.close_generation,
            })
            .map_err(ProcessorControlError::diagnostic)
    }
    fn snapshot_state_for_editor_close(
        &self,
        node: &NodeId,
        value: ports::ProcessorEditorCloseObservation,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError> {
        self.0
            .snapshot_state_for_editor_close(
                node,
                mixrelay_processor_host::ProcessorEditorCloseObservation {
                    instance_id: value.instance_id,
                    close_generation: value.close_generation,
                },
                timeout,
            )
            .map_err(state_error)
    }
    fn processing_time_ns(&self, node: &NodeId) -> Option<u64> {
        self.0.processing_time_ns(node)
    }
    fn latency_snapshot(&self, node: &NodeId) -> Option<ports::ProcessorLatencySnapshot> {
        self.0
            .latency_snapshot(node)
            .map(|value| ports::ProcessorLatencySnapshot {
                transport_frames: value.transport_frames,
                intrinsic_frames: value.intrinsic_frames,
            })
    }
}

fn state_error(error: mixrelay_processor_host::ProcessorStateError) -> ProcessorStateError {
    use mixrelay_processor_host::ProcessorStateError as Host;
    match error {
        Host::Unavailable => ProcessorStateError::Unavailable,
        Host::Busy => ProcessorStateError::Busy,
        Host::Pending => ProcessorStateError::Pending,
        Host::Timeout => ProcessorStateError::Timeout,
        Host::Unsupported => ProcessorStateError::Unsupported,
        Host::NativeFailure => ProcessorStateError::NativeFailure,
        Host::TooLarge => ProcessorStateError::TooLarge,
        Host::InvalidResponse => ProcessorStateError::InvalidResponse,
        Host::Storage => ProcessorStateError::Storage,
    }
}

#[derive(Clone)]
pub(crate) struct HostStateFiles(pub(crate) ProcessorFiles);
impl ports::ProcessorStates for HostStateFiles {
    fn select_namespace(&self, namespace: &Path) {
        self.0.select(mixrelay_processor_state::StateStore::new(
            namespace.to_path_buf(),
        ));
    }
    fn snapshot(&self) -> Result<ports::ProcessorStateSnapshot, ports::ProcessorStorageError> {
        self.0
            .snapshot()
            .map(|store| ports::ProcessorStateSnapshot::new(Arc::new(store)))
            .map_err(|error| match error {
                mixrelay_processor_host::StateStorageError::Unconfigured => {
                    ports::ProcessorStorageError::Unconfigured
                }
                mixrelay_processor_host::StateStorageError::State(source) => {
                    ports::ProcessorStorageError::State(source)
                }
            })
    }
}

#[cfg(test)]
mod tests;
