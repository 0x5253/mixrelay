//! A manually advanced clock for deterministic tests.
use crate::{Deadline, Instant, MonotonicClock, Sleeper};
use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

/// A clock that moves only when a test advances it or the code under test sleeps on it.
///
/// Clones share one reading, so a test can keep a clone while the code under test owns
/// another. Sleeping moves the clock forward to the deadline instead of blocking.
///
/// Drive it from one thread. Advances and sleeps from several threads each move the
/// shared reading, and their order, and so the time each thread sees, is not
/// deterministic.
#[derive(Clone, Debug)]
pub struct ManualClock(Arc<Mutex<Instant>>);

impl Default for ManualClock {
    /// Starts at the clock's zero origin and advances only when driven by the test.
    fn default() -> Self {
        Self::new(Instant::from_origin(Duration::ZERO))
    }
}

impl ManualClock {
    /// A clock that reads `start` until something advances it.
    #[must_use]
    pub fn new(start: Instant) -> Self {
        Self(Arc::new(Mutex::new(start)))
    }

    /// Moves the clock `duration` forward, saturating at the latest representable instant.
    pub fn advance(&self, duration: Duration) {
        let mut now = self.reading();
        *now = now.saturating_add(duration);
    }

    // A panic elsewhere cannot leave the reading half-written, so poisoning is ignored.
    fn reading(&self) -> MutexGuard<'_, Instant> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl MonotonicClock for ManualClock {
    fn now(&self) -> Instant {
        *self.reading()
    }
}

impl Sleeper for ManualClock {
    fn sleep_until(&self, deadline: Deadline) {
        let mut now = self.reading();
        *now = (*now).max(deadline.instant());
    }
}

#[cfg(test)]
mod tests;
