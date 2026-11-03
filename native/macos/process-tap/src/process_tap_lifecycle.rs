//! Capture admission ownership and invalidation gates shared by control and callbacks.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const MAX_OWNED_SESSIONS: u32 = 32;
static OWNED_SESSIONS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn active_owners() -> u32 {
    OWNED_SESSIONS.load(Ordering::Acquire)
}

/// Embedded, zero-initialized session gate. Admission is released only after
/// native callback/listener unregistration, drain and device destruction succeed.
/// Retired sessions retain their admission while the native cleanup queue retries.
#[repr(C)]
#[derive(Debug, Default)]
pub(crate) struct TapLifecycle {
    epoch: AtomicU64,
    validated: AtomicU64,
    retiring: AtomicBool,
    admitted: AtomicBool,
}

const _: () = assert!(size_of::<TapLifecycle>() == 24 && align_of::<TapLifecycle>() == 8);

impl TapLifecycle {
    pub(crate) fn acquire_owner(&self) -> bool {
        self.acquire(&OWNED_SESSIONS)
    }
    pub(crate) fn release_owner(&self) {
        self.release(&OWNED_SESSIONS);
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub(crate) fn retiring(&self) -> bool {
        self.retiring.load(Ordering::Acquire)
    }
    pub(crate) fn delivery_allowed(&self) -> bool {
        self.current(self.epoch())
    }

    fn acquire(&self, owned: &AtomicU32) -> bool {
        if self.retiring.load(Ordering::Acquire) {
            return false;
        }
        if self.admitted.load(Ordering::Relaxed) {
            return true;
        }
        let admitted = owned
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_OWNED_SESSIONS).then(|| count + 1)
            })
            .is_ok();
        self.admitted.store(admitted, Ordering::Relaxed);
        admitted
    }

    fn release(&self, owned: &AtomicU32) {
        if self.admitted.swap(false, Ordering::Relaxed) {
            owned.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn changed(&self) {
        // Exhaustion stays invalid permanently; it must never wrap to an old
        // validated generation and reopen delivery after enough notifications.
        // Native property listeners, not the audio IOProc, may retry this CAS.
        let _ = self
            .epoch
            .fetch_update(Ordering::Release, Ordering::Relaxed, |epoch| {
                Some(epoch.saturating_add(1))
            });
    }

    pub(crate) fn current(&self, observed: u64) -> bool {
        observed != u64::MAX
            && !self.retiring.load(Ordering::Acquire)
            && observed == self.epoch.load(Ordering::Acquire)
            && observed == self.validated.load(Ordering::Acquire)
    }

    pub(crate) fn invalidate(&self, observed: u64) -> u64 {
        if observed == self.validated.load(Ordering::Acquire) {
            let next = observed.saturating_add(1);
            if self
                .epoch
                .compare_exchange(observed, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return next;
            }
        }
        // A notification during native format discovery must not be certified
        // by the stale description. Commit will reject this original generation.
        observed
    }

    pub(crate) fn commit(&self, observed: u64) -> bool {
        if observed == u64::MAX || self.retiring.load(Ordering::Acquire) {
            return false;
        }
        self.validated.store(observed, Ordering::Release);
        self.current(observed)
    }

    pub(crate) fn retire(&self) -> bool {
        !self.retiring.swap(true, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests;
