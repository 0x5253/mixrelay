//! Native file observations and synced atomic replacement for engine documents.

use mixrelay_engine_core::document_files::{
    DocumentCommit, DocumentFiles, StagedWrite, WriteDocument,
};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

/// Selects native document files without reading paths or creating a directory.
#[derive(Debug, Clone, Copy)]
pub struct DiskDocuments;

impl DocumentFiles for DiskDocuments {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(File::open(path)?))
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
    fn stage(&self, path: &Path, write: WriteDocument<'_>) -> io::Result<StagedWrite> {
        let staged = crate::persistence::stage_file(path, write)?;
        Ok(StagedWrite::new(Box::new(DiskCommit(staged))))
    }
}

struct DiskCommit(crate::persistence::StagedWrite);
impl DocumentCommit for DiskCommit {
    fn commit(self: Box<Self>) -> io::Result<()> {
        self.0.commit()
    }
}

#[cfg(test)]
mod tests;
