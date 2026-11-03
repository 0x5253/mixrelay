//! Owned document observations and unpublished transactions for engine policy tests.

use super::{DocumentCommit, DocumentFiles, StagedWrite, WriteDocument};
use parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Default)]
struct Documents {
    accepted: BTreeMap<PathBuf, Vec<u8>>,
    stage_errors: BTreeMap<PathBuf, io::ErrorKind>,
    commit_errors: BTreeMap<PathBuf, io::ErrorKind>,
    remove_errors: BTreeMap<PathBuf, io::ErrorKind>,
}

/// Clones share accepted documents; opening retains one immutable observed version.
#[derive(Debug, Clone, Default)]
pub struct MemoryDocuments(Arc<Mutex<Documents>>);

impl MemoryDocuments {
    #[must_use]
    pub fn accepted_document_count(&self) -> usize {
        self.0.lock().accepted.len()
    }
    pub fn insert(&self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
        self.0.lock().accepted.insert(path.into(), bytes.into());
    }
    #[must_use]
    pub fn bytes(&self, path: &Path) -> Option<Vec<u8>> {
        self.0.lock().accepted.get(path).cloned()
    }
    pub fn reject_stage(&self, path: impl Into<PathBuf>, kind: io::ErrorKind) {
        self.0.lock().stage_errors.insert(path.into(), kind);
    }
    pub fn allow_stage(&self, path: &Path) {
        self.0.lock().stage_errors.remove(path);
    }
    pub fn reject_commit(&self, path: impl Into<PathBuf>, kind: io::ErrorKind) {
        self.0.lock().commit_errors.insert(path.into(), kind);
    }
    pub fn allow_commit(&self, path: &Path) {
        self.0.lock().commit_errors.remove(path);
    }
    pub fn reject_remove(&self, path: impl Into<PathBuf>, kind: io::ErrorKind) {
        self.0.lock().remove_errors.insert(path.into(), kind);
    }
    pub fn allow_remove(&self, path: &Path) {
        self.0.lock().remove_errors.remove(path);
    }
}

impl DocumentFiles for MemoryDocuments {
    fn exists(&self, path: &Path) -> bool {
        self.0.lock().accepted.contains_key(path)
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        self.bytes(path)
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        let mut documents = self.0.lock();
        if let Some(kind) = documents.remove_errors.get(path) {
            return Err(io::Error::from(*kind));
        }
        documents
            .accepted
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn stage(&self, path: &Path, write: WriteDocument<'_>) -> io::Result<StagedWrite> {
        if let Some(kind) = self.0.lock().stage_errors.get(path) {
            return Err(io::Error::from(*kind));
        }
        let mut bytes = Vec::new();
        write(&mut bytes)?;
        Ok(StagedWrite::new(Box::new(MemoryCommit {
            documents: self.clone(),
            path: path.to_owned(),
            bytes,
        })))
    }
}

struct MemoryCommit {
    documents: MemoryDocuments,
    path: PathBuf,
    bytes: Vec<u8>,
}
impl DocumentCommit for MemoryCommit {
    fn commit(self: Box<Self>) -> io::Result<()> {
        let mut documents = self.documents.0.lock();
        if let Some(kind) = documents.commit_errors.get(&self.path) {
            return Err(io::Error::from(*kind));
        }
        documents.accepted.insert(self.path, self.bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
