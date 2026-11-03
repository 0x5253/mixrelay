//! One pump's leased producer endpoint. Storage ownership is shared with the
//! registry implementation; the producer guard serializes remote lease changes.

mod clock;
mod ffi;
pub use clock::{ConsumerTiming, ConsumerTimingRead, SourceClockRead, SourceClockSnapshot};
pub use ffi::{
    TransportError, TransportErrorCode, mr_application_source_transport_attach,
    mr_application_source_transport_clock, mr_application_source_transport_connect,
    mr_application_source_transport_destroy, mr_application_source_transport_publish_measurements,
    mr_application_source_transport_publish_native,
    mr_application_source_transport_record_source_faults, mr_application_source_transport_reset,
    mr_application_source_transport_stop_native, mr_application_source_transport_write,
    mr_application_source_transport_write_clocked, mr_application_source_transport_write_timed,
};

use crate::{
    application_clock::ApplicationClock,
    application_source_mapping::Mapping,
    shared_audio_buffer::{
        mr_shared_audio_buffer_channels, mr_shared_audio_buffer_stats, mr_shared_audio_buffer_write,
    },
};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
enum Publication {
    PcmOnly,
    NativeCredit,
}

/// Opaque C handle exclusively owned by one source pump until its join/drop.
/// Other processes share mapping atomics, never references to this local owner.
pub struct SourceTransport {
    mapping: Mapping,
    next_frame: u64,
    owned_epoch: u64,
    owned_lease: u64,
    /// Retained across transient producer-guard contention, scoped to `owned_epoch`.
    pending_source_faults: u64,
}

struct ProducerClaim<'a>(&'a ApplicationClock);

impl ProducerClaim<'_> {
    fn acquire(clock: &ApplicationClock) -> Option<ProducerClaim<'_>> {
        clock
            .producer_guard
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| ProducerClaim(clock))
    }
}

impl Drop for ProducerClaim<'_> {
    fn drop(&mut self) {
        self.0.producer_guard.store(0, Ordering::Release);
    }
}

impl SourceTransport {
    /// Creates one producer directly from the owning registry on control.
    /// # Errors
    /// Propagates registry admission and transport attachment failures.
    pub fn create(
        registry: &crate::source_registry::SourceRegistry,
        request: &crate::source_control::SourceRequest,
    ) -> std::io::Result<Self> {
        Self::create_attaching(registry, request, |raw| {
            // SAFETY: the registry owns this immutable backing and grants one producer lease.
            unsafe { ffi::attach(raw, request.capacity_frames, request.channels) }
        })
    }
    fn create_attaching(
        registry: &crate::source_registry::SourceRegistry,
        request: &crate::source_control::SourceRequest,
        attach: impl FnOnce(
            &mut crate::source_control::SourceAttachment,
        ) -> Result<Self, TransportError>,
    ) -> std::io::Result<Self> {
        use std::os::fd::IntoRawFd;
        let attachment = registry.attach(request)?;
        let lease = attachment.producer_lease;
        let mut raw = crate::source_control::SourceAttachment {
            descriptor: attachment.descriptor.into_raw_fd(),
            mapped_size: attachment.mapped_size,
            producer_lease: lease,
            source_epoch: attachment.source_epoch,
        };
        attach(&mut raw).map_err(|error| {
            registry.cancel_attachment(lease);
            std::io::Error::other(error)
        })
    }
    fn owns_stream(&self) -> bool {
        // SAFETY: the mapping owns the validated buffer for the entire borrow;
        // statistics touch only shared atomics, never local producer state.
        unsafe { mr_shared_audio_buffer_stats(self.mapping.buffer.as_ptr()) }.epoch
            == self.owned_epoch
            && self.mapping.clock().producer_lease.load(Ordering::Acquire) == self.owned_lease
    }

    fn write(&mut self, samples: &[f32], frames: u32) -> bool {
        self.write_pcm(samples, frames, None)
    }

    pub fn write_pcm(&mut self, samples: &[f32], frames: u32, times: Option<&[u64]>) -> bool {
        self.write_prepared(samples, frames, times, Publication::PcmOnly)
    }

    /// Publishes native-unit frame credit only after the timed PCM commits, under
    /// the same producer claim. Credit cannot expose an unwritten source interval.
    pub fn write_clocked_pcm(&mut self, samples: &[f32], frames: u32, times: &[u64]) -> bool {
        self.write_prepared(samples, frames, Some(times), Publication::NativeCredit)
    }

    fn write_prepared(
        &mut self,
        samples: &[f32],
        frames: u32,
        times: Option<&[u64]>,
        publication: Publication,
    ) -> bool {
        // SAFETY: mapping attachment validated this immutable channel geometry.
        let channels = unsafe { mr_shared_audio_buffer_channels(self.mapping.buffer.as_ptr()) };
        if frames > self.mapping.capacity() || samples.len() != frames as usize * channels as usize
        {
            return false;
        }
        let clock = self.mapping.clock();
        let Some(_claim) = ProducerClaim::acquire(clock) else {
            return false;
        };
        let acknowledged = clock.acknowledged_format.load(Ordering::Acquire);
        if !self.owns_stream()
            || acknowledged == 0
            || acknowledged != clock.format_word.load(Ordering::Acquire)
        {
            return false;
        }
        let Some(next) = self.next_frame.checked_add(u64::from(frames)) else {
            return false;
        };
        let credit = match publication {
            Publication::PcmOnly => None,
            Publication::NativeCredit => {
                let native_rate = clock.native_sample_rate.load(Ordering::Acquire);
                let graph_rate = acknowledged & u64::from(u32::MAX);
                if !(8_000..=192_000).contains(&native_rate)
                    || !(8_000..=192_000).contains(&graph_rate)
                {
                    return false;
                }
                let credit = u128::from(next) * u128::from(native_rate) / u128::from(graph_rate);
                let Ok(credit) = u64::try_from(credit) else {
                    return false;
                };
                Some(credit)
            }
        };
        flush_source_faults(clock, &mut self.pending_source_faults);
        if times.is_some_and(|times| times.len() != frames as usize) {
            return false;
        }
        // SAFETY: claimed producer, retained mapping and matching PCM extent. Untimed
        // writes explicitly invalidate any coordinate left by an unsuccessful attempt.
        unsafe {
            clock.write_times(
                self.mapping.capacity(),
                self.owned_epoch,
                self.next_frame,
                (0..frames as usize).map(|index| times.map_or(0, |times| times[index])),
            );
        }
        // SAFETY: the FFI facade validated the frame extent. This claim is the
        // sole producer; input is borrowed and disjoint from the owned mapping.
        let written = unsafe {
            mr_shared_audio_buffer_write(
                self.mapping.buffer.as_ptr(),
                self.owned_epoch,
                self.next_frame,
                samples.as_ptr(),
                frames,
            )
        };
        if written {
            self.next_frame = next;
            if let Some(credit) = credit.filter(|_| frames != 0) {
                // Release makes committed PCM visible before any graph clock credit.
                clock.native_frames.store(credit, Ordering::Release);
                clock.native_running.store(1, Ordering::Release);
            }
        }
        written
    }

    pub fn record_source_faults(&mut self, epoch: u64, count: u64) -> bool {
        if epoch != self.owned_epoch || !self.owns_stream() {
            return false;
        }
        self.pending_source_faults = self.pending_source_faults.saturating_add(count);
        let clock = self.mapping.clock();
        if let Some(_claim) = ProducerClaim::acquire(clock)
            && self.owns_stream()
        {
            flush_source_faults(clock, &mut self.pending_source_faults);
        }
        true
    }
}

/// Caller holds the producer claim and has validated its actual lease/epoch.
fn flush_source_faults(clock: &ApplicationClock, pending: &mut u64) {
    clock.source_sample_faults.record(std::mem::take(pending));
}

impl Drop for SourceTransport {
    fn drop(&mut self) {
        let clock = self.mapping.clock();
        let claim = ProducerClaim::acquire(clock);
        if claim.is_some() && self.owns_stream() {
            flush_source_faults(clock, &mut self.pending_source_faults);
        }
        // The sole pump drains its writes/resets before Drop. Leases are unique
        // and never reused, so this CAS publishes closure only for our grant.
        // It works even if a remote grant holds the guard and then fails without
        // changing the lease. Epoch invalidation must not prevent lease closure.
        if clock
            .producer_lease
            .compare_exchange(self.owned_lease, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && claim.is_some()
        {
            clock.acknowledged_format.store(0, Ordering::Release);
            clock.native_frames.store(0, Ordering::Release);
            clock.native_running.store(0, Ordering::Release);
        }
        // Mapping drops after the lease closes: unmap first, then close its FD.
    }
}

#[cfg(test)]
mod tests;
