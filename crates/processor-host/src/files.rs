//! File-store ownership shared by native worker startup and workspace composition.

use std::sync::{Arc, RwLock};

use mixrelay_processor_state::{StateRef, StateStore};

use crate::{StateBlobReader, StateBlobWriter, StateBlobs, StateStorageError};

/// One published file-store selection for capture, workers and workspace files.
/// Construction and selection perform no filesystem I/O.
#[derive(Clone, Debug, Default)]
pub struct ProcessorFiles {
    store: Arc<RwLock<Option<StateStore>>>,
}

impl ProcessorFiles {
    /// Publishes the same store to all consumers before restoring a catalog.
    pub fn select(&self, store: StateStore) {
        *self
            .store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(store);
    }

    /// Retains the selected namespace for one outer file or worker operation.
    /// # Errors
    /// Returns an error before a catalog has selected its storage.
    pub fn snapshot(&self) -> Result<StateStore, StateStorageError> {
        self.optional_snapshot()
            .ok_or(StateStorageError::Unconfigured)
    }

    pub(super) fn optional_snapshot(&self) -> Option<StateStore> {
        self.store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl StateBlobReader for ProcessorFiles {
    fn read(&self, reference: &StateRef) -> Result<Vec<u8>, StateStorageError> {
        Ok(self.snapshot()?.read(reference)?)
    }
}

impl StateBlobs for ProcessorFiles {
    fn prepare_write(&self) -> Result<Box<dyn StateBlobWriter>, StateStorageError> {
        Ok(Box::new(self.snapshot()?))
    }
    fn verify(&self, reference: &StateRef) -> Result<(), StateStorageError> {
        Ok(self.snapshot()?.verify(reference)?)
    }
}

impl StateBlobReader for StateStore {
    fn read(&self, reference: &StateRef) -> Result<Vec<u8>, StateStorageError> {
        Ok(self.read(reference)?)
    }
}

impl StateBlobWriter for StateStore {
    fn put(&self, bytes: &[u8]) -> Result<StateRef, StateStorageError> {
        Ok(self.put(bytes)?)
    }
}

#[cfg(test)]
mod tests;
