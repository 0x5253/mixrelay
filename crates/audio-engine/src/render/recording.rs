//! Recording ports: the engine hands each recorder node's block to a sink and
//! never knows whether that sink writes files, feeds a test or discards audio.
use mixrelay_audio_graph::NodeId;
use thiserror::Error;

/// The session a recorder node records into. The channel count is part of it, so a
/// replacement plan with another width prepares a separate session while the previous
/// one keeps recording until the replacement is published or rolled back.
#[must_use]
pub fn recording_session_key(node: &NodeId, channels: u16) -> String {
    format!("{}#recorder-channels={channels}", node.as_str())
}

/// Receives one recorder node's interleaved output.
///
/// Sinks are prepared and dropped on the control thread, never on the audio thread.
pub trait RecordingSink: Send {
    /// Accepts one processed block and its actual processing rate.
    ///
    /// Called on the audio thread once per block while the node is On, whether or not
    /// anything records, so the sink always knows the current rate. A muted node passes
    /// silence, an Off node is skipped and an empty block passes an empty slice. Only one
    /// sink per session is captured at a time. Must not allocate, lock, block or perform I/O.
    fn capture(&self, samples: &[f32], sample_rate: u32);
}

/// A recording sink could not be prepared; the processor being prepared is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct RecordingSinkError(pub String);

/// Prepares recording sinks while a graph is prepared on the control thread.
pub trait RecordingSinks {
    /// Returns the sink for `session` with `channels` interleaved channels. Preparing a
    /// session again must keep its state, so a rebuild or a discarded preparation does
    /// not interrupt it.
    ///
    /// # Errors
    ///
    /// Returns a diagnostic when the sink cannot be prepared, for example because the
    /// session limit is reached or its worker cannot start.
    fn prepare(
        &self,
        session: &str,
        channels: u16,
    ) -> Result<Box<dyn RecordingSink>, RecordingSinkError>;
}
