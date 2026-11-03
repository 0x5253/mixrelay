//! Monotonic time ports for platform-neutral domain code.
//!
//! Domain and application services read time through [`MonotonicClock`] and wait through
//! [`Sleeper`], so their tests drive time explicitly instead of waiting for real time to
//! pass. `StdClock` implements both ports over the standard library's monotonic clock for
//! product entry points, and the `test-util` feature provides a manually advanced
//! `ManualClock` for tests.
mod clock;
mod deadline;
mod instant;
#[cfg(any(test, feature = "test-util"))]
mod manual;
mod sleep;
mod std_clock;

pub use clock::MonotonicClock;
pub use deadline::Deadline;
pub use instant::Instant;
#[cfg(any(test, feature = "test-util"))]
pub use manual::ManualClock;
pub use sleep::Sleeper;
pub use std_clock::StdClock;
