//! Weak instance registration and control-thread access to parameters, state and editors.

use crate::{
    EDITOR_COMMAND_CLOSE, EDITOR_COMMAND_OPEN, EDITOR_FAILURE_DISPLAY_UNAVAILABLE,
    EDITOR_FAILURE_HOST_WINDOW, EDITOR_FAILURE_NO_EDITOR, EDITOR_FAILURE_NONE,
    EDITOR_FAILURE_PLUGIN_REJECTED, EDITOR_FAILURE_WORKER_UNAVAILABLE, EDITOR_STATE_CLOSED,
    EDITOR_STATE_FAILED, EDITOR_STATE_OPEN, EDITOR_STATE_OPENING, PROCESSOR_STATE_TIMEOUT,
    WORKER_FAILED, WORKER_READY, latency, reuse, shared_transport::ControlMailbox, timing,
};
use mixrelay_audio_graph::NodeId;
use mixrelay_control_protocol::{
    PROTOCOL_VERSION, ProcessorEditorFailure, ProcessorEditorState, ProcessorEditorStatus,
};
use mixrelay_render::HostedLatencySource;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
mod tests;

/// Weak registry used by the control plane to update active plug-ins without rebuilding the
/// graph. Weak entries ensure a removed node cannot be kept alive by UI bookkeeping.
type WeakProcessorControls = BTreeMap<NodeId, Vec<Weak<HostedProcessorControl>>>;
type SharedProcessorControls = RwLock<WeakProcessorControls>;

#[derive(Clone)]
pub struct ProcessorControlRegistry {
    pub(super) instances: Arc<crate::instance_id::InstanceIds>,
    pub(super) controls: Arc<SharedProcessorControls>,
    pub(super) state_blobs: Arc<dyn crate::StateBlobs>,
}

impl Default for ProcessorControlRegistry {
    fn default() -> Self {
        Self::new(Arc::new(crate::state_blobs::UnconfiguredBlobs))
    }
}

/// Stable control-plane failures; unsupported formats must not be saved as empty success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessorStateError {
    Unavailable,
    Busy,
    Pending,
    Timeout,
    Unsupported,
    NativeFailure,
    TooLarge,
    InvalidResponse,
    Storage,
}

impl std::fmt::Display for ProcessorStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "processor worker is unavailable",
            Self::Busy => "processor control is busy",
            Self::Pending => "processor state request is still pending",
            Self::Timeout => "processor state export timed out",
            Self::Unsupported => "processor native state export is unsupported",
            Self::NativeFailure => "processor native state export failed",
            Self::TooLarge => "processor state exceeds 64 MiB",
            Self::InvalidResponse => "processor state response is invalid",
            Self::Storage => "processor state could not be stored durably",
        })
    }
}

impl std::error::Error for ProcessorStateError {}

impl ProcessorControlRegistry {
    /// Constructs control policy with the selected state-byte boundary.
    #[must_use]
    pub fn new(state_blobs: Arc<dyn crate::StateBlobs>) -> Self {
        Self {
            controls: Arc::default(),
            instances: Arc::new(crate::instance_id::InstanceIds::new()),
            state_blobs,
        }
    }

    /// Captures native state on the worker's editor/control thread. Call only from a control
    /// thread, before saving or replacing a graph. Native state is authoritative: replace the
    /// provider's parameter object with this snapshot, rather than overlaying stale values.
    /// Hold a [`Self::reuse_transaction`] through capture and the owning publication. The
    /// captured reference temporarily authorizes reuse; failed publication must roll it back.
    ///
    /// # Errors
    /// Returns an error for inactive/crashed workers, concurrent or timed-out requests,
    /// unsupported native state APIs, storage failure, or exports exceeding 64 MiB.
    pub fn snapshot_state(
        &self,
        node_id: &NodeId,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError> {
        let control = self
            .live_control(node_id)
            .map_err(|_| ProcessorStateError::Unavailable)?;
        self.snapshot_control_state(&control, timeout)
    }

    pub(super) fn register(
        &self,
        node_id: &NodeId,
        control: &Arc<HostedProcessorControl>,
    ) -> ProcessorControlRegistration {
        let mut controls = self
            .controls
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = controls.entry(node_id.clone()).or_default();
        entries.retain(|entry| entry.strong_count() != 0);
        let registered = Arc::downgrade(control);
        entries.push(registered.clone());
        ProcessorControlRegistration {
            controls: Arc::downgrade(&self.controls),
            node_id: node_id.clone(),
            registered,
        }
    }

    /// Coalesces a normalized parameter value into the worker's lock-free mailbox.
    ///
    /// # Errors
    ///
    /// Returns a stable diagnostic when the value is invalid or the node is no longer active.
    pub fn set_parameter(&self, node_id: &NodeId, id: u32, value: f64) -> Result<(), String> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("processor parameters must be normalized between 0 and 1".to_owned());
        }
        let control = self.live_control(node_id)?;
        control.set_parameter(id, value)
    }

    /// Returns the live native-editor state for the active instance behind `node_id`.
    ///
    /// # Errors
    ///
    /// Returns an error when the graph no longer owns a worker for the requested node.
    pub fn editor_status(&self, node_id: &NodeId) -> Result<ProcessorEditorStatus, String> {
        let control = self.live_control(node_id)?;
        Ok(control.editor_status(node_id))
    }

    /// Latest sampled input-publication to output-receipt time, including worker scheduling.
    /// Returns no value before measurement, after expiry, or for an inactive/failed instance.
    #[must_use]
    pub fn processing_time_ns(&self, node_id: &NodeId) -> Option<u64> {
        let control = self.live_control(node_id).ok()?;
        if !control.active_owner.load(Ordering::Acquire)
            || control
                .shared
                .header()
                .worker_status
                .load(Ordering::Acquire)
                != WORKER_READY
        {
            return None;
        }
        control.timing.latest()
    }

    /// Publishes an idempotent open/close request to the plug-in worker and returns immediately.
    /// The UI polls [`Self::editor_status`] so a slow or modal third-party editor can never block
    /// the engine's HTTP control thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the graph no longer owns a worker for the requested node.
    pub fn set_editor_open(
        &self,
        node_id: &NodeId,
        open: bool,
    ) -> Result<ProcessorEditorStatus, String> {
        let control = self.live_control(node_id)?;
        control.set_editor_open(open);
        Ok(control.editor_status(node_id))
    }

    /// Reports whether a node still owns a worker, including a failed worker whose
    /// state export must report its error rather than silently reuse an old preset.
    #[must_use]
    pub fn has_worker(&self, node_id: &NodeId) -> bool {
        self.live_control(node_id).is_ok()
    }

    /// Returns the exact active owner only when its worker has failed.
    #[must_use]
    pub fn failed_worker_instance(&self, node_id: &NodeId) -> Option<u64> {
        let control = self.active_control(node_id)?;
        let header = control.shared.header();
        (header.worker_status.load(Ordering::Acquire) == WORKER_FAILED
            || header.shutdown.load(Ordering::Acquire) != 0)
            .then_some(header.instance_id)
    }

    /// Reserves one explicit retry for the exact failed active worker instance.
    /// The cooldown lives with that worker owner, so retired identities are never retained.
    /// # Errors
    /// Rejects absent/healthy/stale workers and retries inside the per-instance cooldown.
    pub fn begin_failed_worker_retry(
        &self,
        node_id: &NodeId,
        expected_instance_id: u64,
        cooldown: Duration,
    ) -> Result<(), &'static str> {
        let control = self
            .active_control(node_id)
            .ok_or("processor is not active")?;
        let header = control.shared.header();
        if header.instance_id != expected_instance_id {
            return Err("processor instance changed");
        }
        if header.worker_status.load(Ordering::Acquire) != WORKER_FAILED
            && header.shutdown.load(Ordering::Acquire) == 0
        {
            return Err("processor worker has not failed");
        }
        let mut last_retry = control
            .last_retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = control.clock.now();
        if last_retry.is_some_and(|previous| now.duration_since(previous) < cooldown) {
            return Err("processor retry is cooling down");
        }
        *last_retry = Some(now);
        Ok(())
    }

    pub(super) fn active_control(&self, node_id: &NodeId) -> Option<Arc<HostedProcessorControl>> {
        self.live_control(node_id)
            .ok()
            .filter(|control| control.active_owner.load(Ordering::Acquire))
    }

    pub(super) fn live_control(
        &self,
        node_id: &NodeId,
    ) -> Result<Arc<HostedProcessorControl>, String> {
        let mut controls = self
            .controls
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = controls
            .get_mut(node_id)
            .ok_or_else(|| format!("processor node is not active: {node_id}"))?;
        entries.retain(|entry| entry.strong_count() != 0);
        let mut active = None;
        let mut prepared = None;
        for control in entries.iter().filter_map(Weak::upgrade) {
            if control.active_owner.load(Ordering::Acquire) {
                if active.is_some() {
                    return Err("processor ownership is changing".to_owned());
                }
                active = Some(Arc::clone(&control));
            }
            prepared = Some(control);
        }
        // An adopted instance keeps its controls while a replacement is prepared. Before any
        // adoption, the latest prepared instance remains available for offline state and editors.
        active
            .or(prepared)
            .ok_or_else(|| format!("processor node is not active: {node_id}"))
    }
}

pub(super) struct ProcessorControlRegistration {
    pub(super) controls: Weak<SharedProcessorControls>,
    pub(super) node_id: NodeId,
    pub(super) registered: Weak<HostedProcessorControl>,
}

impl Drop for ProcessorControlRegistration {
    fn drop(&mut self) {
        let Some(controls) = self.controls.upgrade() else {
            return;
        };
        let mut controls = controls
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove_node = if let Some(entries) = controls.get_mut(&self.node_id) {
            entries.retain(|entry| {
                entry.strong_count() != 0 && !Weak::ptr_eq(entry, &self.registered)
            });
            entries.is_empty()
        } else {
            false
        };
        if remove_node {
            controls.remove(&self.node_id);
        }
    }
}

pub(super) struct HostedProcessorControl {
    pub(super) adoption_identity: mixrelay_render::HostedProcessorIdentity,
    native_format: Option<crate::RackFormat>,
    pub(super) clock: Arc<dyn mixrelay_time::MonotonicClock>,
    // Drop capture's mailbox reference before the primary mapping and its file cleanup.
    capture: Box<dyn crate::state_capture::StateCapture>,
    pub(super) shared: Arc<ControlMailbox>,
    // Keep this after `shared`: its Drop unlinks only after the mailbox mapping closes.
    transport_file_cleanup: SharedTransportFileCleanup,
    pub(super) input_writer: Mutex<()>,
    pub(super) reuse: Mutex<Option<reuse::ReuseSignature>>,
    pub(super) active_owner: AtomicBool,
    last_retry: Mutex<Option<mixrelay_time::Instant>>,
    pub(super) timing: timing::TimingMetrics,
}

impl HostedProcessorControl {
    pub(super) fn new(
        shared: Arc<ControlMailbox>,
        clock: Arc<dyn mixrelay_time::MonotonicClock>,
        capture: Box<dyn crate::state_capture::StateCapture>,
    ) -> Self {
        Self {
            adoption_identity: mixrelay_render::HostedProcessorIdentity::new(),
            native_format: None,
            clock,
            capture,
            shared,
            transport_file_cleanup: SharedTransportFileCleanup::default(),
            input_writer: Mutex::new(()),
            reuse: Mutex::new(None),
            active_owner: AtomicBool::new(false),
            last_retry: Mutex::new(None),
            timing: timing::TimingMetrics::default(),
        }
    }

    pub(super) fn with_format(mut self, format: crate::RackFormat) -> Self {
        self.native_format = Some(format);
        self
    }

    pub(super) fn native_format(&self) -> &'static str {
        self.native_format
            .map_or("unknown", crate::RackFormat::argument)
    }

    pub(super) fn own_transport_file(&self, path: PathBuf) {
        self.transport_file_cleanup.0.get_or_init(|| path);
    }

    pub(super) fn snapshot_state(&self, timeout: Duration) -> Result<Vec<u8>, ProcessorStateError> {
        self.capture.snapshot(timeout.min(PROCESSOR_STATE_TIMEOUT))
    }

    pub(super) fn request_blob(
        &self,
        kind: u32,
        timeout: Duration,
    ) -> Result<Vec<u8>, ProcessorStateError> {
        self.shared.try_lock()?.request_blob(kind, timeout)
    }

    pub(super) fn set_parameter(&self, id: u32, value: f64) -> Result<(), String> {
        // HTTP/Tauri requests may arrive concurrently. Serializing publishers prevents a
        // parameter ID from being paired with another request's value; the worker never locks.
        let _writer = self.shared.lock();
        let header = self.shared.header();
        let count = header.parameter_count.load(Ordering::Relaxed);
        let used = count as usize;
        let existing = header.parameters[..used]
            .iter()
            .position(|slot| slot.id.load(Ordering::Relaxed) == id);
        let index = if let Some(index) = existing {
            index
        } else {
            let Some(slot) = header.parameters.get(used) else {
                return Err("processor parameter mailbox capacity exceeded".to_owned());
            };
            slot.id.store(id, Ordering::Relaxed);
            // A free slot exists, so `count` is below the u32-sized mailbox capacity.
            header.parameter_count.store(count + 1, Ordering::Release);
            used
        };
        let slot = &header.parameters[index];
        slot.value_bits.store(value.to_bits(), Ordering::Release);
        slot.pending.store(true, Ordering::Release);
        header.parameters_pending.store(true, Ordering::Release);
        Ok(())
    }

    pub(super) fn set_editor_open(&self, open: bool) {
        let _writer = self.shared.lock();
        let header = self.shared.header();
        if open
            && header.editor_command.load(Ordering::Acquire) == EDITOR_COMMAND_OPEN
            && matches!(
                header.editor_state.load(Ordering::Acquire),
                EDITOR_STATE_OPEN | EDITOR_STATE_OPENING
            )
            && header.editor_failure.load(Ordering::Acquire) == EDITOR_FAILURE_NONE
        {
            return;
        }
        let next = header
            .editor_sequence
            .load(Ordering::Relaxed)
            .wrapping_add(1);
        header
            .editor_failure
            .store(EDITOR_FAILURE_NONE, Ordering::Relaxed);
        if open {
            header
                .editor_state
                .store(EDITOR_STATE_OPENING, Ordering::Relaxed);
        }
        header.editor_command.store(
            if open {
                EDITOR_COMMAND_OPEN
            } else {
                EDITOR_COMMAND_CLOSE
            },
            Ordering::Relaxed,
        );
        header.editor_sequence.store(next, Ordering::Release);
    }

    pub(super) fn editor_status(&self, node_id: &NodeId) -> ProcessorEditorStatus {
        let header = self.shared.header();
        let worker_ready = header.worker_status.load(Ordering::Acquire) == WORKER_READY;
        let state = if worker_ready {
            match header.editor_state.load(Ordering::Acquire) {
                EDITOR_STATE_CLOSED => ProcessorEditorState::Closed,
                EDITOR_STATE_OPENING => ProcessorEditorState::Opening,
                EDITOR_STATE_OPEN => ProcessorEditorState::Open,
                EDITOR_STATE_FAILED => ProcessorEditorState::Failed,
                _ => ProcessorEditorState::Unavailable,
            }
        } else {
            ProcessorEditorState::Failed
        };
        let raw_failure = if worker_ready {
            header.editor_failure.load(Ordering::Acquire)
        } else {
            EDITOR_FAILURE_WORKER_UNAVAILABLE
        };
        let failure = match raw_failure {
            EDITOR_FAILURE_NO_EDITOR => Some(ProcessorEditorFailure::NoEditor),
            EDITOR_FAILURE_DISPLAY_UNAVAILABLE => Some(ProcessorEditorFailure::DisplayUnavailable),
            EDITOR_FAILURE_HOST_WINDOW => Some(ProcessorEditorFailure::HostWindowFailed),
            EDITOR_FAILURE_PLUGIN_REJECTED => Some(ProcessorEditorFailure::PluginRejected),
            EDITOR_FAILURE_WORKER_UNAVAILABLE => Some(ProcessorEditorFailure::WorkerUnavailable),
            _ => None,
        };
        let width = header.editor_width.load(Ordering::Relaxed);
        let height = header.editor_height.load(Ordering::Relaxed);
        ProcessorEditorStatus {
            protocol_version: PROTOCOL_VERSION,
            node_id: node_id.as_str().to_owned(),
            state,
            width: (width != 0).then_some(width),
            height: (height != 0).then_some(height),
            resizable: header.editor_resizable.load(Ordering::Relaxed) != 0,
            failure,
        }
    }
}

#[derive(Default)]
struct SharedTransportFileCleanup(OnceLock<PathBuf>);

impl Drop for SharedTransportFileCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.get() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl HostedLatencySource for HostedProcessorControl {
    fn frames(&self) -> Option<u32> {
        let header = self.shared.header();
        if header.worker_status.load(Ordering::Acquire) != WORKER_READY
            || header.shutdown.load(Ordering::Acquire) != 0
        {
            return None;
        }
        let intrinsic = latency::decode_intrinsic_frames(
            header.intrinsic_latency_frames.load(Ordering::Acquire),
        )?;
        header.prepared_block_frames().checked_add(intrinsic)
    }
}
