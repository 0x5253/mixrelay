//! Bounded, lifetime-safe refcons for asynchronous AU property callbacks.
//!
//! Native callback user data is an opaque integer token encoded as a pointer. It is never
//! dereferenced. Fixed atomic slots validate the token's non-repeating generation before
//! changing state, so a callback already in flight after listener removal cannot touch freed
//! Rust memory or dirty a later instance that reuses the same slot.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

const SLOT_BITS: u32 = 5;
const SLOT_COUNT: usize = 1 << SLOT_BITS;
const SLOT_MASK: usize = SLOT_COUNT - 1;
const DIRTY: usize = 1;
const MAX_GENERATION: usize = (isize::MAX as usize) >> SLOT_BITS;

static REGISTRY: LatencySignalRegistry = LatencySignalRegistry::new();

/// Acquires one of the fixed callback-signal slots. Exhaustion is conservative: callers must
/// leave latency unknown instead of registering a listener without safe callback ownership.
pub(crate) fn acquire() -> Option<LatencySignalLease<'static>> {
    REGISTRY.acquire()
}

/// Marks a callback's slot dirty only if both its slot and generation still identify a live
/// lease. This is constant-time, allocation-free, and does not acquire a lock.
pub(crate) fn signal(user_data: NonNull<c_void>) {
    REGISTRY.signal_token(user_data.as_ptr().addr());
}

struct LatencySignalRegistry {
    slots: [AtomicUsize; SLOT_COUNT],
    next_generation: AtomicUsize,
}

impl LatencySignalRegistry {
    const fn new() -> Self {
        Self {
            slots: [const { AtomicUsize::new(0) }; SLOT_COUNT],
            next_generation: AtomicUsize::new(0),
        }
    }

    fn acquire(&self) -> Option<LatencySignalLease<'_>> {
        let generation = self
            .next_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current < MAX_GENERATION).then_some(current + 1)
            })
            .ok()?
            .checked_add(1)?;
        let clean = clean_state(generation)?;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .compare_exchange(0, clean, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(LatencySignalLease {
                    registry: self,
                    index,
                    generation,
                });
            }
        }
        None
    }

    fn signal_token(&self, token: usize) {
        if token == 0 || token > isize::MAX as usize {
            return;
        }
        let index = token & SLOT_MASK;
        let generation = token >> SLOT_BITS;
        let Some(clean) = clean_state(generation) else {
            return;
        };
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        let _ = slot.compare_exchange(clean, clean | DIRTY, Ordering::AcqRel, Ordering::Acquire);
    }

    fn release(&self, index: usize, generation: usize) {
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        let Some(clean) = clean_state(generation) else {
            return;
        };
        loop {
            let state = slot.load(Ordering::Acquire);
            if state != clean && state != clean | DIRTY {
                return;
            }
            if slot
                .compare_exchange(state, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    fn is_dirty(&self, index: usize, generation: usize) -> bool {
        self.slots
            .get(index)
            .and_then(|slot| {
                clean_state(generation).map(|clean| slot.load(Ordering::Acquire) == clean | DIRTY)
            })
            .unwrap_or(false)
    }

    fn take_invalidation(&self, index: usize, generation: usize) -> bool {
        let Some(slot) = self.slots.get(index) else {
            return false;
        };
        let Some(clean) = clean_state(generation) else {
            return false;
        };
        slot.compare_exchange(clean | DIRTY, clean, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// Unique lease owning a callback slot until after the native unit is disposed.
pub(crate) struct LatencySignalLease<'a> {
    registry: &'a LatencySignalRegistry,
    index: usize,
    generation: usize,
}

impl LatencySignalLease<'_> {
    pub(crate) fn user_data(&self) -> *mut c_void {
        std::ptr::without_provenance_mut(token_value(self.index, self.generation))
    }

    pub(crate) fn mark_dirty(&self) {
        self.registry
            .signal_token(token_value(self.index, self.generation));
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.registry.is_dirty(self.index, self.generation)
    }

    pub(crate) fn take_invalidation(&self) -> bool {
        self.registry.take_invalidation(self.index, self.generation)
    }
}

impl Drop for LatencySignalLease<'_> {
    fn drop(&mut self) {
        self.registry.release(self.index, self.generation);
    }
}

fn clean_state(generation: usize) -> Option<usize> {
    (generation != 0 && generation <= MAX_GENERATION).then_some(generation.checked_mul(2)? & !DIRTY)
}

fn token_value(index: usize, generation: usize) -> usize {
    (generation << SLOT_BITS) | index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_callback_cannot_dirty_a_reused_slot() {
        let registry = LatencySignalRegistry::new();
        let first = registry.acquire().unwrap();
        let old_token = token_value(first.index, first.generation);
        let old_index = first.index;
        registry.signal_token(old_token);
        assert!(first.is_dirty());
        assert!(first.take_invalidation());
        drop(first);

        let second = registry.acquire().unwrap();
        assert_eq!(second.index, old_index);
        assert_ne!(token_value(second.index, second.generation), old_token);
        registry.signal_token(old_token);
        assert!(!second.take_invalidation());

        registry.signal_token(token_value(second.index, second.generation));
        assert!(second.take_invalidation());
    }

    #[test]
    fn callback_slots_are_bounded_and_generation_never_wraps() {
        let registry = LatencySignalRegistry::new();
        let mut leases = Vec::with_capacity(SLOT_COUNT);
        for _ in 0..SLOT_COUNT {
            leases.push(registry.acquire().unwrap());
        }
        assert!(registry.acquire().is_none());
        drop(leases.pop());
        assert!(registry.acquire().is_some());

        let exhausted = LatencySignalRegistry::new();
        exhausted
            .next_generation
            .store(MAX_GENERATION - 1, Ordering::Relaxed);
        let final_generation = exhausted.acquire().unwrap();
        assert_eq!(final_generation.generation, MAX_GENERATION);
        assert!(exhausted.acquire().is_none());
        drop(final_generation);
        assert!(exhausted.acquire().is_none());
    }
}
