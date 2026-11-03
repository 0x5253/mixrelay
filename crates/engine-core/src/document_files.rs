//! Document streams and atomic publication consumed by catalog transactions.

use serde::Serialize;
use std::{
    io::{self, Read, Write},
    path::Path,
};

/// A complete unpublished document; dropping it cancels only this staging.
pub trait DocumentCommit: Send {
    /// Atomically publishes the staged bytes. A post-publication sync warning is not rollback.
    /// # Errors
    /// Returns a failure only while the accepted destination remains unchanged.
    fn commit(self: Box<Self>) -> io::Result<()>;
}

#[must_use = "commit the staged document or drop it to cancel"]
pub struct StagedWrite(Box<dyn DocumentCommit>);
impl std::fmt::Debug for StagedWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedWrite").finish_non_exhaustive()
    }
}
impl StagedWrite {
    pub fn new(commit: Box<dyn DocumentCommit>) -> Self {
        Self(commit)
    }
    /// # Errors
    /// Reports an unpublished replacement failure, preserving the accepted document.
    pub fn commit(self) -> io::Result<()> {
        self.0.commit()
    }
}

pub type WriteDocument<'a> = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + 'a>;

/// Opens document observations and owns durable staging independently of engine policy.
pub trait DocumentFiles: Send + Sync {
    /// Observes whether a marker exists independently of its readability.
    fn exists(&self, path: &Path) -> bool;
    /// # Errors
    /// Reports missing files and native read-admission failures unchanged.
    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + Send>>;
    /// # Errors
    /// Preserves missing-file and native removal failures for the owning policy.
    fn remove(&self, path: &Path) -> io::Result<()>;
    /// Executes the writer once against private bytes and syncs them before returning.
    /// # Errors
    /// Returns creation, writer or sync failures without replacing the accepted destination.
    fn stage(&self, path: &Path, write: WriteDocument<'_>) -> io::Result<StagedWrite>;
}

/// Stages a complete JSON document, including its trailing newline.
/// # Errors
/// Reports serialization and unpublished staging failures.
pub fn stage_json(
    files: &dyn DocumentFiles,
    path: &Path,
    value: &impl Serialize,
) -> io::Result<StagedWrite> {
    stage_json_limited(files, path, value, u64::MAX)
}

/// Bounds serialized bytes before their destination may be published.
/// # Errors
/// Rejects oversized JSON and preserves native staging errors.
pub fn stage_json_limited(
    files: &dyn DocumentFiles,
    path: &Path,
    value: &impl Serialize,
    limit: u64,
) -> io::Result<StagedWrite> {
    files.stage(
        path,
        Box::new(|output| {
            let mut writer = LimitedWriter {
                inner: output,
                remaining: limit,
            };
            serde_json::to_writer(&mut writer, value).map_err(|error| {
                io::Error::new(error.io_error_kind().unwrap_or(io::ErrorKind::Other), error)
            })?;
            writer.write_all(b"\n")
        }),
    )
}

struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
}
impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "document exceeds size limit",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod memory;
