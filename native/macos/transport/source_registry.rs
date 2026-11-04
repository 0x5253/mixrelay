//! Shared-source mapping ownership and single-consumer clock policy.
//! Control publishes immutable slot pointers; callbacks only claim atomics.
//! Old producer mappings stay isolated after replacement and are reclaimed on
//! control after local readers drain, never on an audio callback.
pub mod content_latency;
pub mod ffi;
mod identity;
mod latency;
mod reads;
mod registration;
mod slots;
mod source;
mod timeline;
mod timestamps;

use crate::{application_clock::ConsumerRoute, application_source_mapping::Mapping};
use identity::Leases;
use mixrelay_time::MonotonicClock;
use slots::{Read, Slots};
use source::Source;
use std::{
    io,
    os::fd::OwnedFd,
    pin::Pin,
    sync::{Mutex, MutexGuard, atomic::Ordering},
};
pub use timestamps::{GraphCredit, PublishedCredit, ReadMiss};

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct SourceClock {
    /// Opaque lease/stream identity, not a raw shared-buffer epoch.
    pub epoch: u64,
    pub native_frames: u64,
    pub native_rate: u32,
    pub graph_rate: u32,
    pub running: u32,
    pub synchronized: u32,
}

/// PCM availability and clock metadata observed under one retained source pin.
#[derive(Clone, Copy, Debug)]
pub struct SourcePcmProgress {
    pub clock: SourceClock,
    pub published_frames: u64,
    pub capacity_frames: u32,
}

/// A producer grant: an owned mapping descriptor and the lease/epoch it writes under.
pub struct Attachment {
    pub descriptor: OwnedFd,
    pub mapped_size: usize,
    pub producer_lease: u64,
    pub source_epoch: u64,
}

/// Control-owned state; every owner here outlives the slots that publish it.
struct Control {
    owners: Vec<Pin<Box<Source>>>,
    leases: Leases,
}

#[cfg(any(test, feature = "test-observation"))]
type TestStorage = fn(usize) -> io::Result<OwnedFd>;

pub struct SourceRegistry {
    clock: Box<dyn MonotonicClock>,
    timebase: mixrelay_realtime::Timebase,
    #[cfg(any(test, feature = "test-observation"))]
    storage: Option<TestStorage>,
    slots: Slots,
    control: Mutex<Control>,
}

impl SourceRegistry {
    /// Creates a registry with explicit scheduling time and host-clock conversion.
    #[must_use]
    pub fn new(clock: Box<dyn MonotonicClock>, timebase: mixrelay_realtime::Timebase) -> Self {
        Self {
            clock,
            timebase,
            #[cfg(any(test, feature = "test-observation"))]
            storage: None,
            slots: Slots::new(),
            control: Mutex::new(Control {
                owners: Vec::new(),
                leases: Leases::new(),
            }),
        }
    }

    /// Injects test backing storage while retaining the production mapping and IPC layout.
    #[cfg(any(test, feature = "test-observation"))]
    #[must_use]
    pub fn with_storage(
        clock: Box<dyn MonotonicClock>,
        timebase: mixrelay_realtime::Timebase,
        storage: TestStorage,
    ) -> Self {
        Self {
            storage: Some(storage),
            ..Self::new(clock, timebase)
        }
    }

    #[cfg_attr(
        not(any(test, feature = "test-observation")),
        expect(
            clippy::unused_self,
            reason = "only test builds inject mapping storage through the registry"
        )
    )]
    fn mapping(&self, capacity: u32, channels: u32) -> io::Result<Mapping> {
        #[cfg(any(test, feature = "test-observation"))]
        if let Some(storage) = self.storage {
            return Mapping::with_storage(capacity, channels, storage);
        }
        Mapping::new(capacity, channels)
    }

    fn control(&self) -> MutexGuard<'_, Control> {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read(&self, token: u64) -> Read<'_> {
        self.slots.read(token)
    }

    /// Publishes how the graph's active plan uses a source to its producer. Returns
    /// false when the identified source is absent.
    pub fn set_consumer_route(&self, token: u64, route: ConsumerRoute) -> bool {
        let read = self.read(token);
        let Some(source) = read.source() else {
            return false;
        };
        // Release publishes the route to the producer's next retention decision.
        source
            .mapping
            .clock()
            .consumer_route
            .store(route.word(), Ordering::Release);
        true
    }

    /// Selects pacing authority without changing PCM, format, or producer ownership.
    pub fn set_clock_master(&self, token: u64, master: bool) -> bool {
        let read = self.read(token);
        let Some(source) = read.source() else {
            return false;
        };
        // Release publishes the pacing choice to the producer before its next conversion.
        source
            .mapping
            .clock()
            .clock_master
            .store(u32::from(master), Ordering::Release);
        true
    }

    /// Whether a prepared producer still has a lease and storage for another stream.
    /// Control checks this before handing queues to a pump, including idle epoch exhaustion.
    pub fn producer_can_rearm(&self, token: u64) -> bool {
        let read = self.read(token);
        read.source()
            .is_some_and(|source| !source.terminal() && !source.closed())
    }
}

#[cfg(test)]
mod tests;
