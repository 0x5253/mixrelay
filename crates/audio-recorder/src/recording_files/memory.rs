//! Owned in-memory destinations for recording conversion and worker policies.

use super::{FileFactory, RecordingStream};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    io::{self, Cursor, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Default)]
pub(crate) struct MemoryFiles {
    files: Arc<Mutex<BTreeMap<PathBuf, MemoryStream>>>,
    rejected: Arc<Mutex<BTreeMap<PathBuf, io::ErrorKind>>>,
}

impl MemoryFiles {
    pub(crate) fn reject_create(&self, path: &Path, kind: io::ErrorKind) {
        self.rejected.lock().insert(path.to_owned(), kind);
    }

    pub(crate) fn bytes(&self, path: impl AsRef<Path>) -> Vec<u8> {
        self.files.lock()[path.as_ref()].0.lock().get_ref().clone()
    }

    pub(crate) fn len(&self) -> usize {
        self.files.lock().len()
    }

    pub(crate) fn reader(&self, path: impl AsRef<Path>) -> hound::WavReader<Cursor<Vec<u8>>> {
        mixrelay_test_support::rf64::open_bytes(self.bytes(path)).unwrap()
    }
}

impl FileFactory for MemoryFiles {
    type Stream = MemoryStream;

    fn create(&mut self, path: &Path) -> io::Result<Self::Stream> {
        if let Some(kind) = self.rejected.lock().get(path).copied() {
            return Err(io::Error::from(kind));
        }
        match self.files.lock().entry(path.to_owned()) {
            Entry::Occupied(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            Entry::Vacant(entry) => {
                let stream = MemoryStream::default();
                entry.insert(stream.clone());
                Ok(stream)
            }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct MemoryStream(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Write for MemoryStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().flush()
    }
}

impl Seek for MemoryStream {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.lock().seek(position)
    }
}

impl RecordingStream for MemoryStream {
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        let length =
            usize::try_from(length).map_err(|_| io::Error::other("file size exhausted"))?;
        self.0.lock().get_mut().resize(length, 0);
        Ok(())
    }
}

impl RecordingStream for io::Cursor<Vec<u8>> {
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        let length =
            usize::try_from(length).map_err(|_| io::Error::other("file size exhausted"))?;
        self.get_mut().resize(length, 0);
        Ok(())
    }
}
