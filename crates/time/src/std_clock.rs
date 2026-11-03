//! The production clock over the standard library's monotonic time.
use crate::{Deadline, Instant, MonotonicClock, Sleeper};

/// The standard library's monotonic clock, measured from when this value was created.
///
/// `std::time::Instant` is available on every supported platform, so no per-OS adapter is
/// needed. Decision code receives the ports; only entry points on the product path create
/// this clock, and tests pass a `ManualClock` instead.
#[derive(Clone, Copy, Debug)]
pub struct StdClock {
    origin: std::time::Instant,
}

impl StdClock {
    /// A clock whose origin is now.
    #[must_use]
    #[expect(
        clippy::new_without_default,
        reason = "a Default would let code reach the real clock without a product entry point choosing it"
    )]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl MonotonicClock for StdClock {
    fn now(&self) -> Instant {
        Instant::from_origin(self.origin.elapsed())
    }
}

impl Sleeper for StdClock {
    fn sleep_until(&self, deadline: Deadline) {
        // `thread::sleep` may measure on another clock, so check this one before returning.
        loop {
            let now = self.now();
            if deadline.has_passed(now) {
                return;
            }
            std::thread::sleep(deadline.remaining(now));
        }
    }
}

#[cfg(test)]
mod tests;
