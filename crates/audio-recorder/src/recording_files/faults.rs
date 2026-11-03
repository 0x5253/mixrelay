//! Scripted stream failures shared by RF64 and worker failure tests.
use super::{
    FileFactory, RecordingStream,
    memory::{MemoryFiles, MemoryStream},
};
use std::{
    io::{self, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

pub(crate) struct FaultIo {
    file: MemoryStream,
    fault: Arc<AtomicU8>,
}

impl Write for FaultIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let fault = self.fault.load(Ordering::Relaxed);
        if matches!(fault, 1 | 5 | 6) {
            if fault == 6 {
                self.fault.store(0, Ordering::Relaxed);
            }
            return Err(io::Error::other("disk full"));
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fault.load(Ordering::Relaxed) == 2 {
            return Err(io::Error::other("flush failed"));
        }
        self.file.flush()
    }
}

impl Seek for FaultIo {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.fault.load(Ordering::Relaxed) == 3 {
            return Err(io::Error::other("seek failed"));
        }
        self.file.seek(position)
    }
}

impl RecordingStream for FaultIo {
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        if matches!(self.fault.load(Ordering::Relaxed), 4 | 5) {
            return Err(io::Error::other("truncate failed"));
        }
        self.file.truncate(length)
    }
}

// Mode 5 fails writes and extent recovery; mode 6 fails one write then permits recovery.
pub(crate) struct FaultFiles(pub(crate) MemoryFiles, pub(crate) Arc<AtomicU8>);
impl FileFactory for FaultFiles {
    type Stream = FaultIo;
    fn create(&mut self, path: &Path) -> io::Result<Self::Stream> {
        Ok(FaultIo {
            file: self.0.create(path)?,
            fault: Arc::clone(&self.1),
        })
    }
}
