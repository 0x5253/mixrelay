//! Processor catalog, control and state assets consumed by engine transactions.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use mixrelay_audio_graph::{NodeId, ProviderPorts};
use mixrelay_control_protocol::{
    ExternalProcessorCatalog, ProcessorEditorFrame, ProcessorEditorInput, ProcessorEditorStatus,
    ProtocolError,
};
use mixrelay_processor_state::{StateBlobSink, StateBlobSource, StateError, StateRef};

/// Shared control-plane budget for an authoritative native-state response.
pub const PROCESSOR_STATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Stable state-export failures interpreted by checkpoint and retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProcessorStateError {
    #[error("processor worker is unavailable")]
    Unavailable,
    #[error("processor control is busy")]
    Busy,
    #[error("processor state request is still pending")]
    Pending,
    #[error("processor state export timed out")]
    Timeout,
    #[error("processor native state export is unsupported")]
    Unsupported,
    #[error("processor native state export failed")]
    NativeFailure,
    #[error("processor state exceeds 64 MiB")]
    TooLarge,
    #[error("processor state response is invalid")]
    InvalidResponse,
    #[error("processor state could not be stored durably")]
    Storage,
}

/// A diagnostic returned by the native catalog or editor adapter.
#[derive(Debug, thiserror::Error)]
#[error("{diagnostic}")]
pub struct ProcessorControlError {
    diagnostic: String,
}

impl ProcessorControlError {
    #[must_use]
    pub fn diagnostic(diagnostic: String) -> Self {
        Self { diagnostic }
    }
}

/// Catalog observations shared by compilation and authored parameter admission.
pub trait ProcessorCatalog: Send + Sync {
    fn ports(&self, type_id: &str) -> Option<ProviderPorts>;
    fn protocol_catalog(&self) -> ExternalProcessorCatalog;
    fn parameter_is_writable(&self, type_id: &str, parameter_id: u32) -> bool;
    /// Validate all paths before replacing the catalog's discovery observation.
    /// # Errors
    /// Rejects an invalid scan destination without scanning any path.
    fn rescan(&self, custom_paths: &[PathBuf]) -> Result<(), ProtocolError>;
}

/// An uncommitted reuse transaction rolls back when its owner is dropped.
pub trait ProcessorReuseTransaction: Send {
    fn commit(self: Box<Self>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorEditorCloseObservation {
    pub instance_id: u64,
    pub close_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorLatencySnapshot {
    pub transport_frames: u32,
    pub intrinsic_frames: Option<u32>,
}

/// Prepared-worker control observations and mutations, outside audio callbacks.
pub trait ProcessorControls: Send + Sync {
    /// # Errors
    /// Rejects missing, busy, failed or unsupported workers and bounded capture or storage failures.
    fn snapshot_state(
        &self,
        node: &NodeId,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError>;
    fn reuse_transaction(&self) -> Box<dyn ProcessorReuseTransaction>;
    fn invalidate_reuse(&self);
    fn has_worker(&self, node: &NodeId) -> bool;
    fn failed_worker_instance(&self, node: &NodeId) -> Option<u64>;
    /// # Errors
    /// Rejects a stale or healthy owner and retries before its cooldown expires.
    fn begin_failed_worker_retry(
        &self,
        node: &NodeId,
        expected_instance: u64,
        cooldown: Duration,
    ) -> Result<(), ProcessorControlError>;
    /// # Errors
    /// Rejects unavailable workers or invalid normalized parameter values.
    fn set_parameter(
        &self,
        node: &NodeId,
        parameter_id: u32,
        value: f64,
    ) -> Result<(), ProcessorControlError>;
    /// # Errors
    /// Rejects nodes without a retained native editor owner.
    fn editor_status(&self, node: &NodeId) -> Result<ProcessorEditorStatus, ProcessorControlError>;
    /// # Errors
    /// Rejects nodes without a retained native editor owner.
    fn set_editor_open(
        &self,
        node: &NodeId,
        open: bool,
    ) -> Result<ProcessorEditorStatus, ProcessorControlError>;
    /// # Errors
    /// Rejects nodes whose native editor owner is unavailable.
    fn cancel_editor_gesture(&self, node: &NodeId) -> Result<(), ProcessorControlError>;
    /// # Errors
    /// Rejects unavailable, replaced or closed editor sessions and bounded capture failures.
    fn editor_frame(&self, node: &NodeId) -> Result<ProcessorEditorFrame, ProcessorControlError>;
    /// # Errors
    /// Rejects stale sessions, invalid input or full native input queues.
    fn editor_input(
        &self,
        node: &NodeId,
        input: &ProcessorEditorInput,
    ) -> Result<(), ProcessorControlError>;
    /// # Errors
    /// Rejects unavailable workers; a crash is not an editor-close observation.
    fn editor_close_observation(
        &self,
        node: &NodeId,
    ) -> Result<ProcessorEditorCloseObservation, ProcessorControlError>;
    /// # Errors
    /// Rejects stale instance or close tokens and authoritative capture failures.
    fn snapshot_state_for_editor_close(
        &self,
        node: &NodeId,
        observation: ProcessorEditorCloseObservation,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError>;
    fn processing_time_ns(&self, node: &NodeId) -> Option<u64>;
    fn latency_snapshot(&self, node: &NodeId) -> Option<ProcessorLatencySnapshot>;
}

/// One retained state namespace supplies bundle sources and receives imported payloads.
pub trait StateAssets: StateBlobSource + StateBlobSink + Send + Sync {}
impl<T: StateBlobSource + StateBlobSink + Send + Sync> StateAssets for T {}

#[derive(Clone)]
pub struct ProcessorStateSnapshot(Arc<dyn StateAssets>);
impl std::fmt::Debug for ProcessorStateSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessorStateSnapshot")
            .finish_non_exhaustive()
    }
}
impl ProcessorStateSnapshot {
    #[must_use]
    pub fn new(assets: Arc<dyn StateAssets>) -> Self {
        Self(assets)
    }
}
impl StateBlobSource for ProcessorStateSnapshot {
    fn verify(&self, reference: &StateRef) -> Result<(), StateError> {
        self.0.verify(reference)
    }
    fn copy_verified(
        &self,
        reference: &StateRef,
        writer: &mut dyn Write,
    ) -> Result<(), StateError> {
        self.0.copy_verified(reference, writer)
    }
}
impl StateBlobSink for ProcessorStateSnapshot {
    fn store_verified(
        &self,
        reader: &mut dyn Read,
        reference: &StateRef,
    ) -> Result<(), StateError> {
        self.0.store_verified(reader, reference)
    }
}

/// Failure to select or retain the state namespace used by future transactions.
#[derive(Debug, thiserror::Error)]
pub enum ProcessorStorageError {
    #[error("processor state store is not configured")]
    Unconfigured,
    #[error(transparent)]
    State(#[from] StateError),
}

pub trait ProcessorStates: Send + Sync {
    /// Publishes the namespace before restoring a catalog, without filesystem I/O.
    fn select_namespace(&self, namespace: &Path);
    /// Retains the current namespace for a complete bundle or catalog transaction.
    /// # Errors
    /// Rejects operations until a catalog selects its state namespace.
    fn snapshot(&self) -> Result<ProcessorStateSnapshot, ProcessorStorageError>;
}
