//! Structured recorder failures retained through finalization and worker shutdown.

use std::{collections::TryReserveError, io, path::PathBuf, sync::Arc};
use thiserror::Error;

/// A shared failure preserves its source when both status and a command report it.
#[derive(Debug, Clone, Error)]
#[error(transparent)]
pub struct RecorderError(Arc<RecorderFailure>);

impl RecorderError {
    /// Returns the failure category and its original source.
    #[must_use]
    pub fn failure(&self) -> &RecorderFailure {
        &self.0
    }

    pub(crate) fn same_failure(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn combine(self, secondary: Self) -> Self {
        if self.contains_failure(&secondary) {
            return self;
        }
        if let RecorderFailure::Combined { primary, secondary } = secondary.failure() {
            return self.combine(primary.clone()).combine(secondary.clone());
        }
        RecorderFailure::Combined {
            primary: self,
            secondary,
        }
        .into()
    }

    fn contains_failure(&self, candidate: &Self) -> bool {
        self.same_failure(candidate)
            || match self.failure() {
                RecorderFailure::Combined { primary, secondary } => {
                    primary.contains_failure(candidate) || secondary.contains_failure(candidate)
                }
                _ => false,
            }
    }
}

impl From<RecorderFailure> for RecorderError {
    fn from(failure: RecorderFailure) -> Self {
        Self(Arc::new(failure))
    }
}

impl From<TryReserveError> for RecorderError {
    fn from(source: TryReserveError) -> Self {
        RecorderFailure::Allocation(source).into()
    }
}

/// Recorder-local rejection, I/O, conversion or worker failure.
#[derive(Debug, Error)]
pub enum RecorderFailure {
    /// The registry accepts a nonempty identity and one to eight channels.
    #[error("invalid recorder identity or channel count")]
    InvalidIdentity,
    /// A live tap or open take retains its channel count.
    #[error("recorder channels cannot change while a tap or take is active")]
    ActiveChannels,
    /// All sixteen session slots remain owned.
    #[error("recorder session capacity exceeded; reap retired taps first")]
    Capacity,
    /// No worker was prepared for the requested identity.
    #[error("recorder identity is not prepared")]
    Unprepared,
    /// Record cannot restart an already recording take.
    #[error("recorder is already recording")]
    AlreadyRecording,
    /// Record requires a live render-plan tap.
    #[error("recorder has no prepared audio tap")]
    NoTap,
    /// The next admission token would wrap or enter the fault bits.
    #[error("recorder admission generation exhausted")]
    AdmissionExhausted,
    /// Resume preserves the original take's path and format.
    #[error("resuming a recording cannot change its path, rate or encoding")]
    ResumeChanged,
    /// An input callback must first publish a supported sample rate.
    #[error("recorder input rate is not observed yet")]
    InputRateUnobserved,
    /// A new take requires a destination.
    #[error("new recording requires a path")]
    MissingPath,
    /// Relative recording destinations are rejected.
    #[error("recording path must be absolute")]
    RelativePath,
    /// The output rate is outside the product rate catalog.
    #[error("unsupported recording sample rate")]
    UnsupportedRate,
    /// Only finite PCM reaches the converter and encoder.
    #[error("non-finite recorder PCM")]
    NonFinitePcm,
    /// A rate segment must exist before accepting PCM.
    #[error("recording segment was not prepared")]
    SegmentUnprepared,
    /// The complete output frame counter cannot advance.
    #[error("recording frame counter overflow")]
    FrameOverflow,
    /// Input frame geometry exceeds the supported counter.
    #[error("recording input length overflow")]
    InputLengthOverflow,
    /// Accumulated input or converted duration exceeds its counter.
    #[error("recording duration overflow")]
    DurationOverflow,
    /// Output frame geometry exceeds the supported counter.
    #[error("recording output length overflow")]
    OutputLengthOverflow,
    /// Conversion requires a prepared resampler.
    #[error("recording converter missing")]
    ConverterMissing,
    /// Conversion emitted more than its exact target duration.
    #[error("converter exceeded exact recording duration")]
    ConversionExceededDuration,
    /// The bounded converter tail did not reach its exact target.
    #[error("recording converter tail did not finish")]
    ConversionTailIncomplete,
    /// Callback admission failed without affecting the audio graph.
    #[error(transparent)]
    Admission(#[from] AdmissionFailure),
    /// Destination creation failed without overwriting existing data.
    #[error("cannot create recording {}: {source}", path.display())]
    CreateFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The RF64 format header could not be created.
    #[error("cannot create RF64 header {}: {source}", path.display())]
    CreateHeader {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    /// The sample writer failed; subsequent writes retain this failure.
    #[error("recording write failed {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    /// Complete-frame extent recovery or header finalization failed.
    #[error("RF64 finalization failed {}: {source}", path.display())]
    Finalize {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The take's destination is already closed.
    #[error("recording {} is already finalized", path.display())]
    Finalized { path: PathBuf },
    /// A converter could not be prepared before callback admission.
    #[error("recording converter preparation failed: {0}")]
    ConverterPreparation(#[source] rubato::ResamplerConstructionError),
    /// Converter processing failed with its original diagnostic.
    #[error(transparent)]
    Conversion(#[from] rubato::ResampleError),
    /// Interleaved converter buffer geometry was invalid.
    #[error(transparent)]
    Geometry(#[from] audioadapter_buffers::SizeError),
    /// Fallible PCM allocation failed off the audio thread.
    #[error(transparent)]
    Allocation(#[from] TryReserveError),
    /// Preparing the callback packet pool failed before publishing a worker.
    #[error("cannot prepare recording PCM: {0}")]
    CaptureAllocation(#[source] TryReserveError),
    /// Preparing the worker's packet failed before publishing it.
    #[error("cannot prepare recording writer PCM: {0}")]
    WriterAllocation(#[source] TryReserveError),
    /// The native worker thread could not be started.
    #[error("cannot start recording writer: {0}")]
    WorkerSpawn(#[source] io::Error),
    /// The command channel closed before delivery.
    #[error("recording writer disconnected")]
    WorkerDisconnected,
    /// The worker stopped before acknowledging a command.
    #[error("recording writer did not acknowledge command")]
    WorkerUnacknowledged,
    /// Joining the worker detected a panic.
    #[error("recording writer panicked")]
    WorkerPanicked,
    /// Both the original operation and finalization failed.
    #[error("{primary}; {secondary}")]
    Combined {
        #[source]
        primary: RecorderError,
        secondary: RecorderError,
    },
    /// Session cleanup retains its identity, destination and failure.
    #[error("recorder {id} [{}]: {source}", path.as_deref().unwrap_or("no file"))]
    Session {
        id: String,
        path: Option<String>,
        #[source]
        source: RecorderError,
    },
    /// Every eligible session is processed before reporting cleanup failures.
    #[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Sessions(Vec<RecorderError>),
}

/// Fixed callback admission faults encoded in the transport gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AdmissionFailure {
    /// No free packet remains in the fixed queue.
    #[error("recorder packet queue overrun")]
    Overrun,
    /// The callback geometry or tagged rate is unsupported.
    #[error("recorder received invalid PCM geometry or sample rate")]
    InvalidPcm,
    /// More than one producer entered the same recording transport.
    #[error("recorder has overlapping audio producers")]
    OverlappingProducers,
}

impl From<AdmissionFailure> for RecorderError {
    fn from(failure: AdmissionFailure) -> Self {
        RecorderFailure::Admission(failure).into()
    }
}
