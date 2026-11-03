//! Clone-shared ownership of native recording sessions.
use crate::{
    RecorderError, RecorderRequest, RecorderState, RecorderStatus, RecorderTap, sessions::Sessions,
    threaded_recording::ThreadedFiles,
};
use std::{fmt, sync::Arc};

/// Control-side owner of at most sixteen file recording workers and fixed packet pools.
/// Clones share sessions. The last owner drains and joins its workers on drop.
/// Preparation, control and registry drop belong off the audio thread.
#[derive(Clone)]
pub struct RecorderRegistry(Arc<Sessions<ThreadedFiles>>);

impl fmt::Debug for RecorderRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecorderRegistry")
            .finish_non_exhaustive()
    }
}

impl RecorderRegistry {
    /// Selects native writer threads and create-new disk files.
    /// Threads and packet pools are prepared only when a session is requested.
    #[must_use]
    pub fn threaded_files() -> Self {
        Self(Arc::new(Sessions::new(ThreadedFiles)))
    }

    /// Prepares a stable tap for a graph-local runtime identity, off the audio thread.
    /// Discarding a failed graph preparation never stops an existing active tap.
    /// # Errors
    /// Rejects invalid identity/channels, active channel changes, capacity or worker failure.
    pub fn prepare(&self, id: &str, channels: u16) -> Result<RecorderTap, RecorderError> {
        self.0.prepare(id, channels)
    }

    /// Returns the latest writer state; an unknown identity is stopped/unconfigured.
    #[must_use]
    pub fn status(&self, id: &str) -> RecorderStatus {
        self.0.status(id)
    }

    /// Retains open takes while their owner temporarily retires render plans.
    #[must_use]
    pub fn open_take_taps(&self) -> Vec<(String, RecorderTap, RecorderState)> {
        self.0.open_take_taps()
    }

    /// Cuts off, drains and acknowledges a writer command on the control thread.
    /// # Errors
    /// Reports identity, transition, file/conversion or worker failures.
    pub fn command(
        &self,
        id: &str,
        request: RecorderRequest,
    ) -> Result<RecorderStatus, RecorderError> {
        self.0.command(id, request)
    }

    /// Drains and joins sessions after all their taps retire.
    /// # Errors
    /// Processes every eligible session before reporting retained cleanup failures.
    pub fn reap_unused(&self) -> Result<(), RecorderError> {
        self.0.reap_unused()
    }

    /// Finalizes every take while retaining stopped taps for existing render plans.
    /// # Errors
    /// Processes every session before reporting failures with identity/path context.
    pub fn stop_all(&self) -> Result<(), RecorderError> {
        self.0.stop_all()
    }
}
