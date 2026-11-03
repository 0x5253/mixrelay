//! A native owner retains its admission slot through failed startup and delayed
//! teardown. Removing a graph node must not bypass the bound on live workers.
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Admission {
    limit: usize,
    active: AtomicUsize,
}
impl Admission {
    pub(super) const fn new(limit: usize) -> Self {
        Self {
            limit,
            active: AtomicUsize::new(0),
        }
    }
    pub(super) fn in_use(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
    pub(super) fn acquire(&self) -> Option<Permit<'_>> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.limit).then(|| active + 1)
            })
            .ok()
            .map(|_| Permit(self))
    }
}
pub(super) struct Permit<'a>(&'a Admission);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_creation_and_final_retirement_release_exactly_one_slot() {
        let admission = Admission::new(2);
        let first = admission.acquire().unwrap();
        let second = admission.acquire().unwrap();
        assert!(admission.acquire().is_none());
        drop(first);
        let replacement = admission.acquire().unwrap();
        assert!(admission.acquire().is_none());
        drop(second);
        drop(replacement);
        assert_eq!(admission.active.load(Ordering::Acquire), 0);
    }
    #[test]
    fn detached_owner_keeps_its_slot_until_its_last_reference_retires() {
        let admission = Admission::new(1);
        let owner = std::sync::Arc::new(admission.acquire().unwrap());
        let worker = owner.clone();
        drop(owner);
        assert!(admission.acquire().is_none());
        drop(worker);
        assert!(admission.acquire().is_some());
    }
    #[test]
    fn concurrent_admission_and_retirement_never_exceed_the_bound() {
        let admission = Admission::new(4);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    for _ in 0..1000 {
                        if let Some(permit) = admission.acquire() {
                            assert!(admission.active.load(Ordering::Acquire) <= 4);
                            std::thread::yield_now();
                            drop(permit);
                        }
                    }
                });
            }
        });
        assert_eq!(admission.active.load(Ordering::Acquire), 0);
    }
}
