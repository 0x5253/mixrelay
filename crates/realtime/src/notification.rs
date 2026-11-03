//! Notification state independent of native semaphore creation and waits.
#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Why a a bounded notification wait returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// A producer published; recheck authoritative state. `published_at` is the host
    /// time of the oldest publication this wake answers.
    Notified { published_at: u64 },
    /// The deadline passed; recheck state anyway, since a wake may have been lost.
    TimedOut,
    /// The kernel refused the wait (its code); the caller should stop relying on it.
    Failed(i32),
}

/// Why a a shared-word wait returned. Every outcome means "recheck the word's state".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexWake {
    /// Woken, or the word no longer held the expected value.
    Changed,
    TimedOut,
    /// The kernel refused the wait (its errno); the caller should poll instead.
    Failed(i32),
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default)]
pub(crate) struct Notifications {
    pending: AtomicU32,
    failed_signals: AtomicU64,
    notified_at: AtomicU64,
}

#[cfg(any(target_os = "macos", test))]
impl Notifications {
    pub(crate) fn publish(&self, now: impl FnOnce() -> u64, signal: impl FnOnce() -> i32) {
        // AcqRel coalesces publications after their release stores and orders the consumer's next reads.
        if self.pending.swap(1, Ordering::AcqRel) == 0 {
            self.notified_at.store(now(), Ordering::Release);
            if signal() != 0 {
                // No token was posted: let the next publication try again.
                self.pending.store(0, Ordering::Release);
                self.failed_signals.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    pub(crate) fn failed_signals(&self) -> u64 {
        self.failed_signals.load(Ordering::Relaxed)
    }
    pub(crate) fn received(&self, result: impl FnOnce(u64) -> Wake) -> Wake {
        let wake = result(self.notified_at.load(Ordering::Acquire));
        if matches!(wake, Wake::Notified { .. }) {
            // Consume the token before authoritative state is read; later publications post a new one.
            self.pending.swap(0, Ordering::AcqRel);
        }
        wake
    }
}

#[cfg(test)]
mod tests;
