//! The port for reading monotonic time.
use crate::Instant;

/// Reads the current time from a monotonic clock.
///
/// The name says "monotonic" to keep this port apart from the audio sample clocks in the
/// product. `Send + Sync` lets control threads and workers share one clock.
pub trait MonotonicClock: Send + Sync {
    /// The current instant. It is never earlier than a previous reading of the same clock.
    fn now(&self) -> Instant;
}
