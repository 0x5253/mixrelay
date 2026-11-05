//! Fixed published-source slots and the reader count that pins them.
//! Readers never lock or allocate; control unpublishes a slot first and frees
//! its owner only once it observes no readers.
use super::source::Source;
use std::{
    ptr,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

pub(super) const MAX_SOURCES: usize = 64;

pub(super) struct Slots {
    slots: [AtomicPtr<Source>; MAX_SOURCES],
    readers: AtomicUsize,
}

/// A reader pin. While it lives, no unpublished owner is reclaimed.
pub(super) struct Read<'a> {
    readers: &'a AtomicUsize,
    source: *const Source,
}

impl Read<'_> {
    pub(super) fn source(&self) -> Option<&Source> {
        // SAFETY: the reader count taken in `Slots::read` pins every owner
        // published at that time until this guard drops.
        unsafe { self.source.as_ref() }
    }
}

impl Drop for Read<'_> {
    fn drop(&mut self) {
        self.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Slots {
    pub(super) fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            readers: AtomicUsize::new(0),
        }
    }

    /// Pins the published source for `token`, if any. Realtime-safe.
    pub(super) fn read(&self, token: u64) -> Read<'_> {
        self.readers.fetch_add(1, Ordering::SeqCst);
        let source = self
            .slots
            .iter()
            .find_map(|slot| {
                let source = slot.load(Ordering::SeqCst);
                // The reader increment pins every pointer before inspection.
                unsafe { source.as_ref() }
                    .filter(|source| source.token == token)
                    .map(ptr::from_ref)
            })
            .unwrap_or(ptr::null());
        Read {
            readers: &self.readers,
            source,
        }
    }

    /// Control only: every published source with its slot index.
    pub(super) fn published(&self) -> impl Iterator<Item = (usize, &Source)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            // SAFETY: control owns every published pointer and frees one only
            // after unpublishing it, under the same control mutex.
            unsafe { slot.load(Ordering::SeqCst).as_ref() }.map(|source| (index, source))
        })
    }

    /// Control only: the published source for `token` and its slot.
    pub(super) fn find(&self, token: u64) -> Option<(usize, &Source)> {
        self.published().find(|(_, source)| source.token == token)
    }

    pub(super) fn vacant(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.load(Ordering::SeqCst).is_null())
    }

    /// Publishes a source that control owns pinned until it is collected.
    pub(super) fn publish(&self, index: usize, source: *const Source) {
        self.slots[index].store(source.cast_mut(), Ordering::SeqCst);
    }

    /// Unpublishes every closed source. Existing pins still retain them.
    pub(super) fn unpublish_closed(&self) {
        for (index, source) in self.published() {
            if source.closed() {
                self.slots[index].store(ptr::null_mut(), Ordering::SeqCst);
            }
        }
    }

    pub(super) fn drained(&self) -> bool {
        self.readers.load(Ordering::SeqCst) == 0
    }

    pub(super) fn contains(&self, source: &Source) -> bool {
        self.slots
            .iter()
            .any(|slot| ptr::eq(slot.load(Ordering::SeqCst), source))
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.published().next().is_none()
    }
}
