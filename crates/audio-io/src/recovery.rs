//! Bounded control-to-audio recovery handoff. Native preparation and destruction
//! stay on control threads; stale generations cannot resurrect removed resources.
use std::{
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

struct Prepared<T> {
    generation: u64,
    value: T,
}

/// Holds one pending and one displaced owner for control-side reclamation.
/// A borrowed [`RecoveryConsumer`] exclusively owns the activation role; it never
/// owns the mailbox allocation or destroys a prepared native resource.
pub struct RecoveryMailbox<T: Send> {
    pending: AtomicPtr<Prepared<T>>,
    retired: AtomicPtr<Prepared<T>>,
    claimed: AtomicBool,
}

impl<T: Send> Default for RecoveryMailbox<T> {
    fn default() -> Self {
        Self {
            pending: AtomicPtr::new(ptr::null_mut()),
            retired: AtomicPtr::new(ptr::null_mut()),
            claimed: AtomicBool::new(false),
        }
    }
}

impl<T: Send> RecoveryMailbox<T> {
    /// Claims the activation role during worker setup, or returns `None` while
    /// another consumer holds it. Dropping that consumer permits a new claim.
    /// The borrow prevents mailbox destruction until the consumer is gone.
    #[must_use]
    pub fn try_consumer(&self) -> Option<RecoveryConsumer<'_, T>> {
        self.claimed
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| RecoveryConsumer { mailbox: self })
    }

    /// Reports whether both publication and control-side retirement slots are empty.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.pending.load(Ordering::Acquire).is_null()
            && self.retired.load(Ordering::Acquire).is_null()
    }

    /// Control thread only. Failed publication retains caller ownership.
    /// # Errors
    /// Returns the prepared value when another recovery is still pending.
    pub fn publish(&self, generation: u64, value: T) -> Result<(), T> {
        let pointer = Box::into_raw(Box::new(Prepared { generation, value }));
        if self
            .pending
            .compare_exchange(
                ptr::null_mut(),
                pointer,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            // SAFETY: failed publication leaves sole ownership on this thread.
            return Err(unsafe { Box::from_raw(pointer) }.value);
        }
        Ok(())
    }

    /// Control thread only; drains displaced and stale native owners.
    pub fn reclaim(&self) {
        let pointer = self.retired.swap(ptr::null_mut(), Ordering::AcqRel);
        if !pointer.is_null() {
            // SAFETY: swap transfers the unique retired owner to this caller.
            drop(unsafe { Box::from_raw(pointer) });
        }
    }
}

/// Exclusive, non-cloneable activation role borrowing a live mailbox.
/// Moving this token does not transfer ownership of any native resource. Its
/// exclusive method borrow prevents simultaneous or reentrant activation.
///
/// ```compile_fail
/// use mixrelay_audio_io::recovery::RecoveryMailbox;
/// let mailbox = RecoveryMailbox::<u64>::default();
/// let mut consumer = mailbox.try_consumer().unwrap();
/// consumer.activate(1, |_| consumer.activate(1, |_| {}));
/// ```
///
/// ```compile_fail
/// use mixrelay_audio_io::recovery::RecoveryMailbox;
/// let consumer = {
///     let mailbox = RecoveryMailbox::<u64>::default();
///     mailbox.try_consumer().unwrap()
/// };
/// drop(consumer);
/// ```
pub struct RecoveryConsumer<'a, T: Send> {
    mailbox: &'a RecoveryMailbox<T>,
}

impl<T: Send> RecoveryConsumer<'_, T> {
    /// Applies a matching generation, then retires its allocation for control
    /// reclamation. Stale generations skip `apply`; an occupied retirement slot
    /// leaves pending work untouched. Even if `apply` panics, the owner is retired.
    ///
    /// Audio callers must keep `apply` bounded, allocation-free and nonblocking,
    /// and swap displaced native owners into `next` instead of destroying them.
    pub fn activate(&mut self, generation: u64, apply: impl FnOnce(&mut T)) {
        if !self.mailbox.retired.load(Ordering::Acquire).is_null() {
            return;
        }
        let pointer = self.mailbox.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if pointer.is_null() {
            return;
        }
        let _retirement = Retirement {
            slot: &self.mailbox.retired,
            pointer,
        };
        // SAFETY: the pending swap acquired its sole Box owner. try_consumer
        // issues one live token, and this exclusive borrow excludes reentry.
        // Retirement publishes only after this reference's last use, including
        // unwinding, so concurrent reclaim cannot free the borrowed value.
        let prepared = unsafe { &mut *pointer };
        if prepared.generation == generation {
            apply(&mut prepared.value);
        }
    }
}

impl<T: Send> Drop for RecoveryConsumer<'_, T> {
    fn drop(&mut self) {
        self.mailbox.claimed.store(false, Ordering::Release);
    }
}

struct Retirement<'a, T> {
    slot: &'a AtomicPtr<Prepared<T>>,
    pointer: *mut Prepared<T>,
}

impl<T> Drop for Retirement<'_, T> {
    fn drop(&mut self) {
        self.slot.store(self.pointer, Ordering::Release);
    }
}

impl<T: Send> Drop for RecoveryMailbox<T> {
    fn drop(&mut self) {
        for slot in [&mut self.pending, &mut self.retired] {
            let pointer = *slot.get_mut();
            if !pointer.is_null() {
                // SAFETY: exclusive destruction after the audio consumer drains.
                drop(unsafe { Box::from_raw(pointer) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Owner(Arc<Mutex<Vec<std::thread::ThreadId>>>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.lock().unwrap().push(std::thread::current().id());
        }
    }
    #[test]
    fn stale_and_replaced_owners_are_destroyed_only_by_control() {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let mailbox = RecoveryMailbox::default();
        assert!(mailbox.publish(1, Owner(drops.clone())).is_ok());
        std::thread::scope(|scope| {
            let mut consumer = mailbox.try_consumer().unwrap();
            scope
                .spawn(move || consumer.activate(2, |_| panic!("stale resource activated")))
                .join()
                .unwrap();
        });
        assert!(drops.lock().unwrap().is_empty());
        mailbox.reclaim();
        assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
        assert!(mailbox.idle());
        assert!(mailbox.publish(3, Owner(drops.clone())).is_ok());
        assert!(mailbox.publish(3, Owner(drops.clone())).is_err());
        let mut active = Owner(drops.clone());
        let mut consumer = mailbox.try_consumer().unwrap();
        consumer.activate(3, |next| std::mem::swap(&mut active, next));
        mailbox.reclaim();
        drop(active);
        assert_eq!(drops.lock().unwrap().len(), 4);
    }
    #[test]
    fn repeated_recovery_is_bounded_and_never_replays_stale_generation() {
        let mailbox = RecoveryMailbox::default();
        let mut consumer = mailbox.try_consumer().unwrap();
        let mut active = 0;
        for generation in 1..1001 {
            mailbox.publish(generation, generation).unwrap();
            consumer.activate(generation, |next| std::mem::swap(&mut active, next));
            assert_eq!(active, generation);
            mailbox.reclaim();
            assert!(mailbox.idle());
        }
    }

    #[test]
    fn recovery_activation_benchmark() {
        if std::env::var_os("MIXRELAY_RECOVERY_BENCH").is_none() {
            return;
        }
        let mailbox = RecoveryMailbox::default();
        let mut consumer = mailbox.try_consumer().unwrap();
        let mut active = 0_u64;
        for pass in 0..8 {
            let start = std::time::Instant::now();
            for _ in 0..1_000_000 {
                consumer.activate(1, std::hint::black_box(|_: &mut u64| {}));
            }
            let idle = start.elapsed();
            let start = std::time::Instant::now();
            for generation in 1..100_001 {
                mailbox.publish(generation, generation).unwrap();
                consumer.activate(generation, |next| std::mem::swap(&mut active, next));
                mailbox.reclaim();
            }
            let recovery = start.elapsed();
            assert_eq!(active, 100_000);
            if pass != 0 {
                eprintln!(
                    "recovery pass={pass} idle_ns_per_call={} cycle_ns_per_call={}",
                    idle.as_nanos() / 1_000_000,
                    recovery.as_nanos() / 100_000
                );
            }
        }
    }

    #[test]
    fn one_consumer_claim_is_enforced_across_threads_and_released_on_drop() {
        let mailbox = RecoveryMailbox::<u64>::default();
        let consumer = mailbox.try_consumer().unwrap();
        assert!(mailbox.try_consumer().is_none());
        std::thread::scope(|scope| {
            scope.spawn(|| assert!(mailbox.try_consumer().is_none()));
        });
        drop(consumer);
        assert!(mailbox.try_consumer().is_some());
    }

    #[test]
    fn concurrent_claims_issue_exactly_one_live_consumer() {
        const CONTENDERS: usize = 8;
        let mailbox = RecoveryMailbox::<u64>::default();
        let start = std::sync::Barrier::new(CONTENDERS);
        let claimed = std::sync::Barrier::new(CONTENDERS);
        let winners = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..CONTENDERS {
                scope.spawn(|| {
                    start.wait();
                    let consumer = mailbox.try_consumer();
                    if consumer.is_some() {
                        winners.fetch_add(1, Ordering::Relaxed);
                    }
                    // Retain the winner until every contender has attempted its
                    // claim; sequential reclaims would not test exclusivity.
                    claimed.wait();
                    drop(consumer);
                });
            }
        });
        assert_eq!(winners.load(Ordering::Relaxed), 1);
        assert!(mailbox.try_consumer().is_some());
    }

    #[test]
    fn retirement_backpressure_preserves_pending_owner_across_consumer_replacement() {
        let mailbox = RecoveryMailbox::default();
        let mut consumer = mailbox.try_consumer().unwrap();
        mailbox.publish(1, 10).unwrap();
        consumer.activate(2, |_| panic!("stale generation activated"));
        mailbox.publish(3, 30).unwrap();
        consumer.activate(3, |_| panic!("occupied retired slot overwritten"));
        assert_eq!(mailbox.publish(4, 40), Err(40));
        drop(consumer);
        let mut consumer = mailbox.try_consumer().unwrap();
        consumer.activate(3, |_| panic!("replacement bypassed backpressure"));
        mailbox.reclaim();
        let mut active = 0;
        consumer.activate(3, |next| std::mem::swap(&mut active, next));
        assert_eq!(active, 30);
        mailbox.reclaim();
        assert!(mailbox.idle());
    }

    #[test]
    fn panicking_activation_returns_owner_for_control_reclamation() {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let mailbox = RecoveryMailbox::default();
        assert!(mailbox.publish(1, Owner(drops.clone())).is_ok());
        std::thread::scope(|scope| {
            let mut consumer = mailbox.try_consumer().unwrap();
            let result = scope.spawn(move || {
                consumer.activate(1, |_| panic!("activation failed"));
            });
            assert!(result.join().is_err());
        });
        assert!(drops.lock().unwrap().is_empty());
        assert!(mailbox.try_consumer().is_some());
        mailbox.reclaim();
        assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
        assert!(mailbox.idle());
    }

    #[test]
    fn consumer_drop_neither_allocates_nor_destroys_mailbox_owners() {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let mailbox = RecoveryMailbox::default();
        assert!(mailbox.publish(1, Owner(drops.clone())).is_ok());
        let mut consumer = mailbox.try_consumer().unwrap();
        let measured = allocation_counter::measure(|| {
            consumer.activate(2, |_| panic!("stale owner activated"));
            drop(consumer);
        });
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.bytes_total, 0);
        assert_eq!(measured.count_current, 0);
        assert!(drops.lock().unwrap().is_empty());
        drop(mailbox);
        assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
    }

    #[test]
    fn matching_activation_does_not_allocate_or_free_the_displaced_owner() {
        let mailbox = RecoveryMailbox::default();
        mailbox.publish(1, Box::new(20_u64)).unwrap();
        let mut active = Box::new(10_u64);
        let mut consumer = mailbox.try_consumer().unwrap();
        let measured = allocation_counter::measure(|| {
            consumer.activate(1, |next| std::mem::swap(&mut active, next));
        });
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.bytes_total, 0);
        assert_eq!(measured.count_current, 0);
        assert_eq!(*active, 20);
        mailbox.reclaim();
    }

    #[test]
    fn concurrent_control_reclaim_waits_for_the_activation_borrow_to_end() {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let mailbox = RecoveryMailbox::default();
        assert!(mailbox.publish(1, Owner(drops.clone())).is_ok());
        let entered = std::sync::Barrier::new(2);
        let finish = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let mut consumer = mailbox.try_consumer().unwrap();
            let (entered, finish) = (&entered, &finish);
            let audio = scope.spawn(move || {
                consumer.activate(1, |owner| {
                    entered.wait();
                    finish.wait();
                    assert!(owner.0.lock().unwrap().is_empty());
                });
            });
            entered.wait();
            mailbox.reclaim();
            assert!(drops.lock().unwrap().is_empty());
            finish.wait();
            audio.join().unwrap();
        });
        assert!(drops.lock().unwrap().is_empty());
        mailbox.reclaim();
        assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
    }
}
