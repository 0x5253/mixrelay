//! Fixed COM event slots with bounded admission for a retained native interface.

use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use truce_rack_core::error::{Error, Result};
use vst3::{
    Class,
    Steinberg::{
        Vst::{Event, IEventList, IEventListTrait},
        kInvalidArgument, kResultOk,
    },
};

pub(super) fn interface_error() -> Error {
    Error::Other("VST3 event list interface unavailable".into())
}

pub(super) struct EventStorage {
    slots: Box<[UnsafeCell<MaybeUninit<Event>>]>,
    count: AtomicUsize,
    frames: AtomicUsize,
    access: AtomicBool,
    faults: AtomicUsize,
    capacity_faults: AtomicUsize,
    writable: bool,
}

// SAFETY: all slot reads and writes require the single-attempt access gate. A
// competing or reentrant native call fails without touching the cells. COM owns
// a reference to this storage through every call, including after adapter drop.
unsafe impl Sync for EventStorage {}

struct Release<'a>(&'a AtomicBool);
impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl EventStorage {
    pub(super) fn new(writable: bool) -> Result<Self> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(super::MAX_EVENTS)
            .map_err(|_| Error::Other("VST3 event storage reservation failed".into()))?;
        slots.resize_with(super::MAX_EVENTS, || UnsafeCell::new(MaybeUninit::uninit()));
        Ok(Self {
            slots: slots.into_boxed_slice(),
            count: AtomicUsize::new(0),
            frames: AtomicUsize::new(0),
            access: AtomicBool::new(false),
            faults: AtomicUsize::new(0),
            capacity_faults: AtomicUsize::new(0),
            writable,
        })
    }

    fn access<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        // Acquire pairs with the previous holder's release before accessing slots.
        if self
            .access
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.faults.fetch_add(1, Ordering::Release);
            return None;
        }
        let release = Release(&self.access);
        let value = operation();
        drop(release);
        Some(value)
    }

    pub(super) fn reset(&self, frames: usize) -> bool {
        self.access(|| {
            self.count.store(0, Ordering::Relaxed);
            self.frames.store(frames, Ordering::Relaxed);
        })
        .is_some()
    }

    pub(super) fn push(&self, event: Event) -> bool {
        self.access(|| {
            let index = self.count.load(Ordering::Relaxed);
            let Some(slot) = self.slots.get(index) else {
                self.capacity_faults.fetch_add(1, Ordering::Release);
                self.faults.fetch_add(1, Ordering::Release);
                return false;
            };
            if event.busIndex < 0
                || event.sampleOffset < 0
                || usize::try_from(event.sampleOffset)
                    .is_ok_and(|offset| offset >= self.frames.load(Ordering::Relaxed))
                || !event.ppqPosition.is_finite()
                || (self.writable && !super::output::valid(event))
            {
                self.faults.fetch_add(1, Ordering::Release);
                return false;
            }
            // SAFETY: the access gate excludes slot readers/writers. This index is
            // inside the reserved storage; the release count publishes its contents.
            unsafe {
                (*slot.get()).write(event);
            }
            self.count.store(index + 1, Ordering::Release);
            true
        })
        .unwrap_or(false)
    }

    pub(super) fn for_each(&self, mut visit: impl FnMut(Event) -> bool) -> bool {
        self.access(|| {
            let count = self.count.load(Ordering::Relaxed);
            for slot in &self.slots[..count] {
                // SAFETY: every published slot is initialized and the access gate
                // excludes native readers/writers for this bounded traversal.
                if !visit(unsafe { (*slot.get()).assume_init_read() }) {
                    return false;
                }
            }
            true
        })
        .unwrap_or(false)
    }

    pub(super) fn capacity_faults(&self) -> usize {
        self.capacity_faults.load(Ordering::Acquire)
    }

    pub(super) fn faults(&self) -> usize {
        self.faults.load(Ordering::Acquire)
    }
}

impl Class for EventStorage {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventStorage {
    unsafe fn getEventCount(&self) -> i32 {
        // The capacity is smaller than i32::MAX; acquire observes the published count.
        i32::try_from(self.count.load(Ordering::Acquire)).unwrap_or(0)
    }

    unsafe fn getEvent(&self, index: i32, out: *mut Event) -> i32 {
        if out.is_null() || !out.is_aligned() {
            return kInvalidArgument;
        }
        self.access(|| {
            let Some(slot) = usize::try_from(index)
                .ok()
                .filter(|index| *index < self.count.load(Ordering::Relaxed))
                .and_then(|index| self.slots.get(index))
            else {
                return kInvalidArgument;
            };
            // SAFETY: the count covers only initialized slots; the gate excludes
            // concurrent mutation. The native caller provides a writable Event.
            unsafe {
                *out = (*slot.get()).assume_init_read();
            }
            kResultOk
        })
        .unwrap_or(kInvalidArgument)
    }

    unsafe fn addEvent(&self, event: *mut Event) -> i32 {
        if !self.writable || event.is_null() || !event.is_aligned() {
            self.faults.fetch_add(1, Ordering::Release);
            return kInvalidArgument;
        }
        // SAFETY: the native caller supplies one readable Event for this call.
        // Pointer payloads are copied as opaque metadata and never dereferenced.
        if self.push(unsafe { *event }) {
            kResultOk
        } else {
            kInvalidArgument
        }
    }
}

#[cfg(test)]
mod tests;
