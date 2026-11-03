use crate::record::Record;
use serde::Serialize;
use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Slot {
    ready: AtomicBool,
    value: UnsafeCell<MaybeUninit<Record>>,
}

// SAFETY: one reservation owns each slot forever; it publishes after writing.
// Readers acquire ready before copying, and published values are never changed.
unsafe impl Sync for Slot {}

pub(crate) struct Journal<const N: usize> {
    next: AtomicUsize,
    slots: [Slot; N],
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    pub(crate) reserved: usize,
    pub(crate) dropped: usize,
    pub(crate) incomplete: usize,
    pub(crate) records: Vec<Record>,
}

impl<const N: usize> Journal<N> {
    pub(crate) const fn new() -> Self {
        Self {
            next: AtomicUsize::new(0),
            slots: [const {
                Slot {
                    ready: AtomicBool::new(false),
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                }
            }; N],
        }
    }

    // Before enabling the hooks, write each slot's atomic on the loader thread.
    // Slot stride is below a page; this faults in every journal page off RT.
    pub(crate) fn prefault(&self) {
        for slot in &self.slots {
            slot.ready.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) fn push(&self, record: &Record) {
        let index = self.next.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = self.slots.get(index) {
            // SAFETY: unique monotonically reserved index, no slot reuse. This
            // finite lab process cannot issue usize::MAX events before exit.
            unsafe { (*slot.value.get()).write(*record) };
            slot.ready.store(true, Ordering::Release);
        }
    }

    // Off RT only. In-flight/unpublished records are counted, never read.
    pub(crate) fn snapshot(&self) -> Snapshot {
        let reserved = self.next.load(Ordering::Acquire);
        let mut records = Vec::with_capacity(reserved.min(N));
        for slot in self.slots.iter().take(reserved.min(N)) {
            if slot.ready.load(Ordering::Acquire) {
                // SAFETY: release/acquire publishes a complete immutable Copy
                // value. No writer can revisit this slot during serialization.
                records.push(unsafe { (*slot.value.get()).assume_init() });
            }
        }
        Snapshot {
            reserved,
            dropped: reserved.saturating_sub(N),
            incomplete: reserved.min(N) - records.len(),
            records,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Operation;
    #[test]
    fn concurrent_producers_preserve_complete_unique_events_and_account_for_exhaustion() {
        let journal = Journal::<64>::new();
        journal.prefault();
        // SAFETY: null fixture snapshot does not access PipeWire.
        let sample = unsafe {
            Record::snapshot(
                Operation::Queue,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                false,
            )
        };
        std::thread::scope(|scope| {
            for source in 0..4 {
                let journal = &journal;
                scope.spawn(move || {
                    for sequence in 0..16 {
                        let mut record = sample;
                        record.stream = source * 16 + sequence;
                        journal.push(&record);
                    }
                });
            }
        });
        let result = journal.snapshot();
        assert_eq!(result.reserved, 64);
        assert_eq!(result.incomplete, 0);
        assert_eq!(result.dropped, 0);
        let ids: std::collections::BTreeSet<_> =
            result.records.iter().map(|record| record.stream).collect();
        assert_eq!(ids.len(), 64);
        let allocation = allocation_counter::measure(|| journal.push(&sample));
        assert_eq!(allocation.count_total, 0);
        assert_eq!(journal.snapshot().dropped, 1);
        // A reserved but unfinished writer must not expose uninitialized data.
        let unfinished = Journal::<1>::new();
        unfinished.next.store(1, Ordering::Relaxed);
        assert_eq!(unfinished.snapshot().incomplete, 1);
    }
}
