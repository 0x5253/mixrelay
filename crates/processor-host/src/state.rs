//! Durable native presets stay outside graph JSON and private IPC configuration.
//! Capture and restoration use the same state-byte port outside render callbacks.

use crate::{
    HostedProcessorControl, ProcessorControlRegistry, ProcessorStateError, StateBlobReader,
    StateStorageError,
};
use mixrelay_processor_state::StateRef;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub(super) enum StatePreparationError {
    #[error("inline processor state is not supported")]
    InlineState,
    #[error(transparent)]
    Reference(#[from] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] StateStorageError),
}

pub(super) struct InitialState {
    pub(super) parameters: Value,
    pub(super) native: Option<Vec<u8>>,
}

impl InitialState {
    pub(super) fn resolve(
        parameters: Value,
        store: Option<&dyn StateBlobReader>,
    ) -> Result<Self, StatePreparationError> {
        let native = reference(&parameters)?
            .map(|reference| {
                store
                    .ok_or(StateStorageError::Unconfigured)?
                    .read(&reference)
            })
            .transpose()?;
        Ok(Self { parameters, native })
    }
}

fn reference(parameters: &Value) -> Result<Option<StateRef>, StatePreparationError> {
    if parameters.get("stateBase64").is_some() {
        return Err(StatePreparationError::InlineState);
    }
    parameters
        .get("stateRef")
        .map(|value| serde_json::from_value(value.clone()).map_err(StatePreparationError::from))
        .transpose()
}

impl ProcessorControlRegistry {
    pub(super) fn verify_state(&self, parameters: &Value) -> Result<(), StatePreparationError> {
        if let Some(reference) = reference(parameters)? {
            self.state_blobs.verify(&reference)?;
        }
        Ok(())
    }

    pub(super) fn snapshot_control_state(
        &self,
        control: &HostedProcessorControl,
        timeout: Duration,
    ) -> Result<Value, ProcessorStateError> {
        let state = self.capture_control_state(control, timeout)?;
        checkpoint_state(control, &state)?;
        Ok(state)
    }

    pub(super) fn capture_control_state(
        &self,
        control: &HostedProcessorControl,
        timeout: Duration,
    ) -> Result<Value, ProcessorStateError> {
        let destination = self
            .state_blobs
            .prepare_write()
            .map_err(|_| ProcessorStateError::Storage)?;
        let bytes = control.snapshot_state(timeout)?;
        let reference = destination
            .put(&bytes)
            .map_err(|_| ProcessorStateError::Storage)?;
        Ok(serde_json::json!({"stateRef": reference}))
    }
}

// Preparation compares the candidate against this reference. The graph owner's
// reuse transaction restores the old signature if later publication fails;
// durable blob storage alone does not mean the authored graph accepted this preset.
pub(super) fn checkpoint_state(
    control: &HostedProcessorControl,
    state: &Value,
) -> Result<(), ProcessorStateError> {
    if let Some(signature) = control
        .reuse
        .lock()
        .map_err(|_| ProcessorStateError::Unavailable)?
        .as_mut()
    {
        signature.checkpoint(state);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
