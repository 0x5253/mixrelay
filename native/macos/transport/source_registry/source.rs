//! One published source: its mapping, its consumer timeline and the
//! nonblocking claims that serialize producer handoff and graph reads.
use super::{
    Attachment, content_latency::Measurements, identity::SourceIdentity, timeline::Timeline,
};
use crate::{
    application_clock::{ApplicationClock, mr_application_clock_request_rate},
    application_source_mapping::Mapping,
    shared_audio_buffer::{
        mr_shared_audio_buffer_request_epoch, mr_shared_audio_buffer_requires_replacement,
        mr_shared_audio_buffer_stats, mr_shared_audio_buffer_try_begin_epoch,
    },
};
use std::{
    cell::UnsafeCell,
    io,
    marker::PhantomPinned,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) struct Source {
    pub(super) id: Vec<u8>,
    pub(super) token: u64,
    pub(super) capacity: u32,
    pub(super) channels: u32,
    pub(super) mapping: Mapping,
    consumer_busy: AtomicBool,
    pub(super) timeline: UnsafeCell<Timeline>,
    pub(super) measurements: Measurements,
    // Published slot addresses cannot change while any callback retains a pin.
    _pinned: PhantomPinned,
}
// SAFETY: immutable fields and mapping atomics are shared. Only a successful
// consumer_busy claim permits timeline mutation; failed claims never wait.
unsafe impl Send for Source {}
unsafe impl Sync for Source {}

/// The graph's exclusive read claim; dropping it releases the source.
pub(super) struct Consumer<'a>(&'a Source);
impl Drop for Consumer<'_> {
    fn drop(&mut self) {
        self.0.consumer_busy.store(false, Ordering::Release);
    }
}

/// Control's exclusive producer claim; it excludes a concurrent handoff.
pub(super) struct Producer<'a>(&'a ApplicationClock);
impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.0.producer_guard.store(0, Ordering::Release);
    }
}

impl Source {
    pub(super) fn new(identity: SourceIdentity, mapping: Mapping) -> Self {
        Self {
            id: identity.id,
            token: identity.token,
            capacity: identity.capacity,
            channels: identity.channels,
            mapping,
            consumer_busy: AtomicBool::new(false),
            timeline: UnsafeCell::new(Timeline::default()),
            measurements: Measurements::default(),
            _pinned: PhantomPinned,
        }
    }

    /// Whether a request names this source with the same geometry.
    pub(super) fn matches(&self, identity: &SourceIdentity) -> bool {
        self.id == identity.id
            && self.capacity == identity.capacity
            && self.channels == identity.channels
    }

    /// Complete fallible allocation before changing either producer lease.
    pub(super) fn grant(&self, lease: u64, previous: Option<&Self>) -> io::Result<Attachment> {
        // Duplicate before any lease/epoch mutation: XPC construction and an
        // immediate subsequent replacement cannot invalidate this owned grant.
        let descriptor = self.mapping.descriptor.try_clone()?;
        let epoch = unsafe { mr_shared_audio_buffer_try_begin_epoch(self.mapping.buffer.as_ptr()) };
        if epoch == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "source stream could not rearm",
            ));
        }
        if let Some(old) = previous {
            self.take_over(old);
        }
        let clock = self.mapping.clock();
        clock.acknowledged_format.store(0, Ordering::Release);
        clock.native_running.store(0, Ordering::Release);
        clock.producer_lease.store(lease, Ordering::Release);
        Ok(Attachment {
            descriptor,
            mapped_size: self.mapping.bytes,
            producer_lease: lease,
            source_epoch: epoch,
        })
    }

    /// Carries the replaced mapping's rate, pacing role and faults over, then
    /// revokes its producer. Runs only after every fallible grant step.
    fn take_over(&self, old: &Self) {
        let clock = self.mapping.clock();
        let old_clock = old.mapping.clock();
        // The low format-word half carries the requested rate.
        let [r0, r1, r2, r3, ..] = old_clock.format_word.load(Ordering::Acquire).to_le_bytes();
        let rate = u32::from_le_bytes([r0, r1, r2, r3]);
        if rate != 0 {
            unsafe {
                mr_application_clock_request_rate(self.mapping.clock.as_ptr(), rate);
            }
        }
        clock.clock_master.store(
            old_clock.clock_master.load(Ordering::Acquire),
            Ordering::Release,
        );
        // The old producer claim excludes new additions. A graph reader
        // already pinning the old mapping may drain concurrently; atomic
        // take assigns those replacements to that reader or this mapping,
        // never both.
        clock
            .source_sample_faults
            .record(old_clock.source_sample_faults.take());
        old_clock.acknowledged_format.store(0, Ordering::Release);
        old_clock.native_running.store(0, Ordering::Release);
        old_clock.producer_lease.store(0, Ordering::Release);
        // SAFETY: attach holds this source's producer claim and its mapping
        // remains pinned. Revoke old PCM as well as the lease before publishing
        // the replacement, including an active (not yet requested) MAX epoch.
        unsafe {
            mr_shared_audio_buffer_request_epoch(old.mapping.buffer.as_ptr());
        }
    }

    pub(super) fn claim_consumer(&self) -> Option<Consumer<'_>> {
        self.consumer_busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Consumer(self))
    }

    pub(super) fn claim_producer(&self) -> Option<Producer<'_>> {
        let clock = self.mapping.clock();
        clock
            .producer_guard
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Producer(clock))
    }

    /// The producer closed this source; collection may unpublish it.
    pub(super) fn closed(&self) -> bool {
        self.mapping.clock().producer_lease.load(Ordering::Acquire) == 0
    }

    /// The mapping can never carry another stream and needs a replacement.
    pub(super) fn terminal(&self) -> bool {
        (unsafe { mr_shared_audio_buffer_requires_replacement(self.mapping.buffer.as_ptr()) })
            || unsafe { mr_shared_audio_buffer_stats(self.mapping.buffer.as_ptr()) }.epoch
                == u64::from(u16::MAX)
            || self.mapping.clock().format_word.load(Ordering::Acquire) >> 32 == u64::from(u32::MAX)
    }

    /// Recheck the nonzero lease/format/epoch observed before copying PCM. The
    /// read pin retains this mapping even when control unpublishes its slot.
    pub(super) fn read_is_current(&self, lease: u64, format: u64, epoch: u64) -> bool {
        let clock = self.mapping.clock();
        clock.acknowledged_format.load(Ordering::Acquire) == format
            && lease == clock.producer_lease.load(Ordering::Acquire)
            && epoch == unsafe { mr_shared_audio_buffer_stats(self.mapping.buffer.as_ptr()) }.epoch
    }
}
