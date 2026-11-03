//! Recording streams and create-new destinations consumed by take conversion.

use std::{
    io::{self, Seek, Write},
    path::Path,
};

pub(crate) trait RecordingStream: Write + Seek {
    fn truncate(&mut self, length: u64) -> io::Result<()>;
}

/// Creates one new destination without replacing an existing recording.
pub(crate) trait FileFactory {
    type Stream: RecordingStream;
    fn create(&mut self, path: &Path) -> io::Result<Self::Stream>;
}

#[cfg(test)]
pub(crate) mod memory;

#[cfg(test)]
pub(crate) mod faults;
