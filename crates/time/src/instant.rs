//! Points in monotonic time, compared without reading a clock.
use std::time::Duration;

/// A point on one monotonic clock, measured from that clock's origin.
///
/// Each clock chooses its own origin, so compare only instants read from the same
/// [`MonotonicClock`](crate::MonotonicClock). Arithmetic saturates instead of panicking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(Duration);

impl Instant {
    /// The instant `offset` after the clock's origin. Clock adapters and test clocks use
    /// this to express their readings.
    #[must_use]
    pub const fn from_origin(offset: Duration) -> Self {
        Self(offset)
    }

    /// Time from `earlier` to `self`, or zero when `earlier` is the later instant.
    #[must_use]
    pub fn duration_since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    /// The instant `duration` after `self`, or the latest representable instant when the
    /// sum overflows.
    #[must_use]
    pub fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration))
    }
}

#[cfg(test)]
mod tests;
