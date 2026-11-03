//! Verified in-memory state for control-policy tests.

use crate::{StateBlobReader, StateBlobWriter, StateBlobs, StateStorageError};
use mixrelay_processor_state::{MAX_STATE_BYTES, StateError, StateRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Default)]
pub(crate) struct MemoryBlobs {
    blobs: Arc<Mutex<BTreeMap<StateRef, Vec<u8>>>>,
    reject_writes: Arc<AtomicBool>,
}

impl MemoryBlobs {
    pub(crate) fn reject_writes(&self) {
        self.reject_writes.store(true, Ordering::Relaxed);
    }
}

impl StateBlobWriter for MemoryBlobs {
    fn put(&self, bytes: &[u8]) -> Result<StateRef, StateStorageError> {
        if self.reject_writes.load(Ordering::Relaxed) {
            return Err(
                StateError::Io(std::io::Error::other("injected blob write failure")).into(),
            );
        }
        if bytes.len() > MAX_STATE_BYTES {
            return Err(StateError::InvalidReference {
                reason: "state exceeds the maximum byte length",
            }
            .into());
        }
        let reference = StateRef::new(format!("{:x}", Sha256::digest(bytes)), bytes.len() as u64)?;
        self.blobs
            .lock()
            .unwrap()
            .insert(reference.clone(), bytes.to_vec());
        Ok(reference)
    }
}

impl StateBlobReader for MemoryBlobs {
    fn read(&self, reference: &StateRef) -> Result<Vec<u8>, StateStorageError> {
        reference.validate()?;
        self.blobs
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .ok_or(StateError::MissingBlob.into())
    }
}

impl StateBlobs for MemoryBlobs {
    fn prepare_write(&self) -> Result<Box<dyn StateBlobWriter>, StateStorageError> {
        Ok(Box::new(self.clone()))
    }
    fn verify(&self, reference: &StateRef) -> Result<(), StateStorageError> {
        self.read(reference).map(|_| ())
    }
}
