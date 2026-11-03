//! Native-editor close checkpoints belong to a particular live worker instance. The control
//! owner remembers only successfully persisted observations and retries failed exports/writes.
//! Reading an observation is non-consuming; no native state or render-thread work happens here.

use std::{sync::atomic::Ordering, time::Duration};

use crate::{NodeId, ProcessorControlRegistry, ProcessorStateError, WORKER_READY};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorEditorCloseObservation {
    pub instance_id: u64,
    pub close_generation: u64,
}

impl ProcessorControlRegistry {
    /// Reads actual editor close events, including native window-manager close buttons.
    /// Generation zero means this worker has not yet observed an open editor close.
    ///
    /// # Errors
    /// Returns an error for inactive, stopped, or crashed workers; crashes are not closes.
    pub fn editor_close_observation(
        &self,
        node_id: &NodeId,
    ) -> Result<ProcessorEditorCloseObservation, String> {
        let control = self.live_control(node_id)?;
        let header = control.shared.header();
        if header.worker_status.load(Ordering::Acquire) != WORKER_READY
            || header.shutdown.load(Ordering::Acquire) != 0
        {
            return Err("processor worker is unavailable".to_owned());
        }
        Ok(ProcessorEditorCloseObservation {
            instance_id: header.instance_id,
            close_generation: header.editor_close_generation.load(Ordering::Acquire),
        })
    }

    /// Exports state only for the exact instance and observed close requested by the owner.
    /// The owner must persist the returned state before acknowledging this observation.
    /// Hold [`Self::reuse_transaction`] until that durable publication succeeds.
    ///
    /// # Errors
    /// Rejects replaced instances or a newer close event, as well as normal state export errors.
    /// The exported state is not committed here; a rejected result leaves the observation retryable.
    pub fn snapshot_state_for_editor_close(
        &self,
        node_id: &NodeId,
        observation: ProcessorEditorCloseObservation,
        timeout: Duration,
    ) -> Result<serde_json::Value, ProcessorStateError> {
        let control = self
            .live_control(node_id)
            .map_err(|_| ProcessorStateError::Unavailable)?;
        let header = control.shared.header();
        if observation.close_generation == 0
            || header.instance_id != observation.instance_id
            || header.editor_close_generation.load(Ordering::Acquire)
                != observation.close_generation
        {
            return Err(ProcessorStateError::Unavailable);
        }
        let state = self.capture_control_state(&control, timeout)?;
        // Retaining the old Arc permits safe capture but must not permit publishing it into a
        // newer graph instance under the same node ID. Also fence close/reopen/close races.
        if self
            .editor_close_observation(node_id)
            .map_err(|_| ProcessorStateError::Unavailable)?
            != observation
        {
            return Err(ProcessorStateError::Unavailable);
        }
        crate::state::checkpoint_state(&control, &state)?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests;
