//! State bytes consumed by capture, validation and worker restoration.

use mixrelay_processor_state::{StateError, StateRef};

/// Failure to access the selected processor-state namespace.
#[derive(Debug, thiserror::Error)]
pub enum StateStorageError {
    #[error("processor state store is not configured")]
    Unconfigured,
    #[error(transparent)]
    State(#[from] StateError),
}

/// Verified bytes consumed during worker restoration.
pub trait StateBlobReader: Send + Sync {
    /// Reads verified bytes for an admitted reference.
    /// # Errors
    /// Returns an error when storage is unavailable or the bytes fail verification.
    fn read(&self, reference: &StateRef) -> Result<Vec<u8>, StateStorageError>;
}

/// State verification and capture destinations consumed by the control plane.
pub trait StateBlobs: Send + Sync {
    /// Retains the destination before requesting native state from a worker.
    /// # Errors
    /// Returns an error when no destination is available.
    fn prepare_write(&self) -> Result<Box<dyn StateBlobWriter>, StateStorageError>;
    /// Verifies that an admitted reference can be restored.
    /// # Errors
    /// Returns an error when the referenced bytes are unavailable or invalid.
    fn verify(&self, reference: &StateRef) -> Result<(), StateStorageError>;
}

/// The destination retained for one native capture, including concurrent store selection.
pub trait StateBlobWriter: Send {
    /// Interns a payload before its reference can enter a graph.
    /// # Errors
    /// Returns an error when the payload cannot be stored durably.
    fn put(&self, bytes: &[u8]) -> Result<StateRef, StateStorageError>;
}

pub(super) struct UnconfiguredBlobs;

impl StateBlobs for UnconfiguredBlobs {
    fn prepare_write(&self) -> Result<Box<dyn StateBlobWriter>, StateStorageError> {
        Err(StateStorageError::Unconfigured)
    }
    fn verify(&self, _: &StateRef) -> Result<(), StateStorageError> {
        Err(StateStorageError::Unconfigured)
    }
}

#[cfg(test)]
pub(crate) mod memory;
