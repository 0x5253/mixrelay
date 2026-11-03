//! The end of a timed wait or timeout.
use crate::Instant;
use std::time::Duration;

/// The instant at which a timed wait or timeout ends, on the clock that produced it.
///
/// Deadlines order earliest first. A deadline has passed once the clock reads its instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Deadline(Instant);

impl Deadline {
    /// A deadline at `instant`.
    #[must_use]
    pub const fn at(instant: Instant) -> Self {
        Self(instant)
    }

    /// A deadline `timeout` after `now`. A timeout past the latest representable instant
    /// saturates there, so the deadline does not pass while the clock is in range.
    #[must_use]
    pub fn after(now: Instant, timeout: Duration) -> Self {
        Self(now.saturating_add(timeout))
    }

    /// The instant at which the deadline passes.
    #[must_use]
    pub const fn instant(self) -> Instant {
        self.0
    }

    /// Time left at `now`, or zero once the deadline has passed.
    #[must_use]
    pub fn remaining(self, now: Instant) -> Duration {
        self.0.duration_since(now)
    }

    /// Whether the clock has reached the deadline at `now`.
    #[must_use]
    pub fn has_passed(self, now: Instant) -> bool {
        now >= self.0
    }
}

#[cfg(test)]
mod tests;
