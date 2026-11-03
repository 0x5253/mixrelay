//! Session preference storage consumed by native format transactions.
use mixrelay_audio_io::session::SessionFormat;
use std::io;

/// A prepared preference replacement. Dropping it cancels unpublished storage.
pub trait StagedSessionPreferences: Send {
    /// Publishes the staged document after native negotiation succeeds.
    /// # Errors
    /// Returns the storage failure without accepting a different document.
    fn commit(self: Box<Self>) -> io::Result<()>;
}

/// Bounded preference access for one backend's selected namespace.
pub trait SessionPreferences: Send + Sync {
    /// Reads a validated saved format, or absence.
    /// # Errors
    /// Reports malformed or unavailable storage.
    fn load(&self) -> io::Result<Option<SessionFormat>>;
    /// Prepares a replacement without publishing it.
    /// # Errors
    /// Reports storage preparation failure.
    fn stage(&self, format: SessionFormat) -> io::Result<Box<dyn StagedSessionPreferences>>;
}
