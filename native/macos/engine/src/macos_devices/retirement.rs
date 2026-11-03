//! Control-side deferred release. Production lists are process-owned and bounded
//! by admission permits retained inside each resource, including failed releases.
use std::sync::Mutex;

pub(super) trait Retirable {
    /// true means native ownership was released; false leaves it unchanged.
    fn try_release(&mut self) -> bool;
}

pub(super) struct Retirement<T> {
    pending: Mutex<Vec<T>>,
}
impl<T: Retirable> Retirement<T> {
    pub(super) const fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn retire(&self, mut owner: T) {
        if !owner.try_release() {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(owner);
        }
    }

    /// One attempt per retained owner, never a retry thread/timer per failure.
    /// The caller supplies cadence and must not call this from an audio callback.
    pub(super) fn reclaim(&self) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain_mut(|owner| !owner.try_release());
    }
}

#[cfg(test)]
mod tests {
    use super::super::admission::{Admission, Permit};
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    struct Resource {
        ready: Arc<AtomicBool>,
        dropped: Arc<AtomicUsize>,
    }
    impl Retirable for Resource {
        fn try_release(&mut self) -> bool {
            self.ready.load(Ordering::Acquire)
        }
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            assert!(
                self.ready.load(Ordering::Acquire),
                "native owner freed without acknowledgement"
            );
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[test]
    fn failed_retirement_retains_owners_and_success_releases_each_once() {
        let pending = Retirement::new();
        let ready = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicUsize::new(0));
        for _ in 0..32 {
            pending.retire(Resource {
                ready: ready.clone(),
                dropped: dropped.clone(),
            });
        }
        for _ in 0..100 {
            pending.reclaim();
        }
        assert_eq!(pending.pending.lock().unwrap().len(), 32);
        assert_eq!(dropped.load(Ordering::Acquire), 0);
        ready.store(true, Ordering::Release);
        pending.reclaim();
        pending.reclaim();
        assert_eq!(dropped.load(Ordering::Acquire), 32);
        assert!(pending.pending.lock().unwrap().is_empty());
    }
    #[test]
    fn successful_immediate_release_never_enters_the_retry_list() {
        let pending = Retirement::new();
        let dropped = Arc::new(AtomicUsize::new(0));
        pending.retire(Resource {
            ready: Arc::new(AtomicBool::new(true)),
            dropped: dropped.clone(),
        });
        assert!(pending.pending.lock().unwrap().is_empty());
        assert_eq!(dropped.load(Ordering::Acquire), 1);
    }

    #[test]
    fn failed_release_keeps_admission_until_native_acknowledgement() {
        struct Owner<'a> {
            ready: Arc<AtomicBool>,
            _permit: Permit<'a>,
        }
        impl Retirable for Owner<'_> {
            fn try_release(&mut self) -> bool {
                self.ready.load(Ordering::Acquire)
            }
        }
        let admission = Admission::new(1);
        let pending = Retirement::new();
        let ready = Arc::new(AtomicBool::new(false));
        pending.retire(Owner {
            ready: ready.clone(),
            _permit: admission.acquire().unwrap(),
        });
        for _ in 0..128 {
            pending.reclaim();
            assert!(admission.acquire().is_none());
        }
        ready.store(true, Ordering::Release);
        pending.reclaim();
        assert_eq!(admission.in_use(), 0);
    }
}
