//! Disk-backed recording creation and buffered extent recovery.

use crate::recording_files::{FileFactory, RecordingStream};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write},
    path::Path,
};

pub(crate) struct DiskFiles;
impl FileFactory for DiskFiles {
    type Stream = BufWriter<File>;
    fn create(&mut self, path: &Path) -> io::Result<Self::Stream> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(BufWriter::new)
    }
}
impl RecordingStream for File {
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        self.set_len(length)
    }
}
impl<W: RecordingStream> RecordingStream for BufWriter<W> {
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        // Flush before shortening so buffered bytes cannot reappear after finalization.
        self.flush()?;
        self.get_mut().truncate(length)
    }
}

#[cfg(test)]
mod tests;
