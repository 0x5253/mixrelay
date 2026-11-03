//! The port for timed waits.
use crate::Deadline;

/// Blocks the calling thread until a deadline passes.
///
/// A deadline is only meaningful on the clock that produced it, so a sleeper pairs with
/// one [`MonotonicClock`](crate::MonotonicClock); usually one value implements both.
/// Never call it from an audio callback.
///
/// The wait cannot be interrupted, so use it only where nothing needs to stop the thread
/// before the deadline. A loop that must react to stop requests needs a stop-aware wait
/// port, which this crate does not provide yet.
pub trait Sleeper: Send + Sync {
    /// Returns once `deadline` has passed on the paired clock, and at once if it already
    /// has. It never returns early.
    fn sleep_until(&self, deadline: Deadline);
}
