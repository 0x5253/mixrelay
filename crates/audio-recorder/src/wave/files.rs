//! RF64 sample writes, sticky failure and finalization over an injected stream.

use crate::{RecorderError, RecorderFailure, recording_files::FileFactory};
use hound::{Sample, WavSpec};
use std::path::{Path, PathBuf};

mod container;
use container::WaveWriter;

/// Owns only the RF64 file. Take duration, resampling and dither stay outside.
pub(super) struct WaveFile<F: FileFactory> {
    writer: Option<WaveWriter<F::Stream>>,
    path: PathBuf,
    error: Option<RecorderError>,
}

impl<F: FileFactory> WaveFile<F> {
    pub(super) fn new(path: &Path, spec: WavSpec, factory: &mut F) -> Result<Self, RecorderError> {
        let stream = factory
            .create(path)
            .map_err(|error| RecorderFailure::CreateFile {
                path: path.to_owned(),
                source: error,
            })?;
        let writer =
            WaveWriter::new(stream, spec).map_err(|error| RecorderFailure::CreateHeader {
                path: path.to_owned(),
                source: error,
            })?;
        Ok(Self {
            writer: Some(writer),
            path: path.to_owned(),
            error: None,
        })
    }

    pub(super) fn write_sample<S: Sample>(&mut self, sample: S) -> Result<(), RecorderError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(RecorderFailure::Finalized {
                path: self.path.clone(),
            }
            .into());
        };
        let result = writer.write_sample(sample).map_err(|error| {
            RecorderError::from(RecorderFailure::Write {
                path: self.path.clone(),
                source: error,
            })
        });
        if let Err(error) = &result {
            self.error = Some(error.clone());
        }
        result
    }

    pub(super) fn finalize(&mut self) -> Result<(), RecorderError> {
        let finalization = self.writer.take().map_or(Ok(()), |writer| {
            writer.finalize().map_err(|error| {
                RecorderError::from(RecorderFailure::Finalize {
                    path: self.path.clone(),
                    source: error,
                })
            })
        });
        match (self.error.take(), finalization) {
            (None, result) => result,
            (Some(error), Ok(())) => Err(error),
            (Some(error), Err(finalization)) => Err(error.combine(finalization)),
        }
    }
}

#[cfg(test)]
mod tests;
