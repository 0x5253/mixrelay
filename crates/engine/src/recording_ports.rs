//! Recording commands and retained take ownership consumed by engine policy.

use mixrelay_audio_io::recording::{RecorderRequest, RecorderState, RecorderStatus};
use std::{error::Error, sync::Arc};

/// Adapter failure retains its original structured source through engine cleanup.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct RecordingError(#[source] Arc<dyn Error + Send + Sync>);
impl RecordingError {
    #[must_use]
    pub fn new(source: impl Error + Send + Sync + 'static) -> Self {
        Self(Arc::new(source))
    }
}
impl RecordingError {
    #[must_use]
    pub fn original(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.0.as_ref()
    }
}

/// A held take keeps its callback admission owner alive across an empty paused graph.
pub trait RecordingHold: Send + Sync {}

pub struct OpenRecording {
    pub id: String,
    pub hold: Box<dyn RecordingHold>,
    pub state: RecorderState,
}
impl std::fmt::Debug for OpenRecording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRecording")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Recorder lifecycle operations remain independent from graph preparation.
pub trait RecordingBackend: Send + Sync {
    fn status(&self, id: &str) -> RecorderStatus;
    fn open_take_taps(&self) -> Vec<OpenRecording>;
    /// # Errors
    /// Returns invalid transitions and native writer, conversion or file failures.
    fn command(&self, id: &str, request: RecorderRequest)
    -> Result<RecorderStatus, RecordingError>;
    /// # Errors
    /// Returns all recorder-local drain or finalization failures after processing every session.
    fn stop_all(&self) -> Result<(), RecordingError>;
    /// # Errors
    /// Returns cleanup failures after reclaiming every eligible retired session.
    fn reap_unused(&self) -> Result<(), RecordingError>;
}
