//! Durable catalog storage, state-blob verification and write warnings.

use std::{collections::BTreeSet, path::PathBuf};

use mixrelay_processor_state::{StateBlobSource, StateRef};
use parking_lot::Mutex;

use super::{
    Catalog, GraphDocument, MAX_BYTES, ProtocolError, document_write_error, state_assets,
    unavailable,
};
use crate::document_files::{DocumentFiles, StagedWrite, stage_json_limited};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Persistence {
    Durable,
    SessionOnly,
}

struct Store {
    path: PathBuf,
    persistence: Persistence,
    value: Result<Catalog, String>,
}

#[derive(Default)]
pub(crate) struct CatalogStorage {
    store: Mutex<Option<Store>>,
    verified: Mutex<BTreeSet<StateRef>>,
    warning: Mutex<Option<String>>,
}

impl CatalogStorage {
    pub(crate) fn initialize(
        &self,
        path: PathBuf,
        persistence: Persistence,
        value: Result<Catalog, String>,
    ) {
        *self.store.lock() = Some(Store {
            path,
            persistence,
            value,
        });
    }

    /// Inspects one immutable committed value without exposing its lock or storage policy.
    pub(crate) fn inspect<R>(&self, read: impl FnOnce(Option<&Result<Catalog, String>>) -> R) -> R {
        let store = self.store.lock();
        read(store.as_ref().map(|store| &store.value))
    }

    pub(crate) fn read<R>(&self, read: impl FnOnce(&Catalog) -> R) -> Result<R, ProtocolError> {
        self.inspect(|value| {
            let value = value.ok_or_else(|| unavailable("graph catalog not enabled"))?;
            value.as_ref().map(read).map_err(|error| unavailable(error))
        })
    }

    pub(crate) fn candidate(&self, document: &GraphDocument) -> Option<Catalog> {
        self.inspect(|value| {
            // An unreadable saved file protects disk without preventing in-memory edits.
            let mut candidate = value?.as_ref().ok()?.clone();
            candidate.update_document(document.clone());
            Some(candidate)
        })
    }

    pub(crate) fn replace(&self, candidate: Option<Catalog>) {
        if let Some(candidate) = candidate
            && let Some(store) = self.store.lock().as_mut()
        {
            store.value = Ok(candidate);
        }
    }

    pub(crate) fn allow_persistence(&self) {
        if let Some(store) = self.store.lock().as_mut() {
            store.persistence = Persistence::Durable;
        }
    }

    pub(crate) fn writes_disabled(&self) -> bool {
        self.store
            .lock()
            .as_ref()
            .is_some_and(|store| store.persistence == Persistence::SessionOnly)
    }

    /// Stages durable data after the controller's admission checks. The caller serializes
    /// staging, native publication and memory replacement with the graph mutation lock.
    pub(crate) fn stage<S: StateBlobSource>(
        &self,
        files: &dyn DocumentFiles,
        candidate: &Catalog,
        state_source: impl FnOnce() -> Result<S, ProtocolError>,
    ) -> Result<Option<StagedWrite>, ProtocolError> {
        let (path, accepted) = {
            let store = self.store.lock();
            let store = store
                .as_ref()
                .ok_or_else(|| unavailable("graph catalog not enabled"))?;
            if store.persistence == Persistence::SessionOnly {
                return Ok(None);
            }
            let accepted = store
                .value
                .as_ref()
                .ok()
                .map(state_assets::referenced)
                .transpose()?
                .unwrap_or_default();
            (store.path.clone(), accepted)
        };
        let references = state_assets::referenced(candidate)?;
        let mut verified = self.verified.lock();
        let next_verified = if references.is_empty() {
            BTreeSet::new()
        } else {
            let states = state_source()?;
            state_assets::verify_catalog_delta(&references, &accepted, &verified, |reference| {
                states
                    .verify(reference)
                    .map_err(|error| super::invalid(&error.to_string()))
            })?
        };
        let staged = stage_json_limited(files, &path, candidate, MAX_BYTES)
            .map_err(|error| document_write_error(&error))?;
        *verified = next_verified;
        Ok(Some(staged))
    }

    /// A commit after native publication records a durability warning without rejecting
    /// the already accepted audio edit or abandoning its prepared resources.
    pub(crate) fn finish(&self, staged: Option<StagedWrite>) {
        if let Some(staged) = staged {
            self.record_warning(staged.commit().err().map(|error| error.to_string()));
        }
    }

    pub(crate) fn warning(&self) -> Option<String> {
        self.warning.lock().clone()
    }

    pub(crate) fn record_warning(&self, warning: Option<String>) {
        *self.warning.lock() = warning;
    }
}

#[cfg(feature = "test-util")]
impl CatalogStorage {
    pub(crate) fn test_set_path(&self, path: PathBuf) {
        if let Some(store) = self.store.lock().as_mut() {
            store.path = path;
        }
    }
}

/// Lock observations for test doubles that call back into the controller.
#[cfg(test)]
mod lock_observation {
    impl super::CatalogStorage {
        /// Whether no caller holds the catalog store, so a callback may read it.
        pub(crate) fn is_unlocked(&self) -> bool {
            self.store.try_lock().is_some()
        }
    }
}
