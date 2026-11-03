//! Consumer snapshots, producer epoch acknowledgment and native clock metadata.
use super::{ProducerClaim, SourceTransport, flush_source_faults};
use crate::{
    application_clock::{ConsumerRoute, FRAME_MASK, NO_PHASE},
    shared_audio_buffer::mr_shared_audio_buffer_try_begin_epoch,
};
use std::sync::atomic::Ordering;

/// A coherent snapshot, a transient consumer publication, or a revoked producer grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceClockRead {
    Ready(SourceClockSnapshot),
    Deferred,
    Revoked,
}

/// How the consumer reads this epoch, as far as its last coherent publication shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConsumerTimingRead {
    /// It reads by time, with this clock against the frame timestamps.
    Timed(ConsumerTiming),
    /// It reads in sequence.
    Sequential,
    /// It is publishing, or has not read this epoch yet: keep its last mode.
    Unknown,
}

/// A time-addressed consumer's clock against this epoch's frame timestamps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConsumerTiming {
    /// Nanoseconds per graph frame the consumer requests, over its continuous run.
    pub period_ns: f64,
    /// Nanoseconds from its requested time to its next unread frame, within one frame.
    pub phase_ns: Option<i32>,
}

/// Coherent consumer observation and the local producer position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct SourceClockSnapshot {
    /// Requested generation and sample rate, encoded by the clock contract.
    pub format_word: u64,
    /// Current mapping-local stream generation.
    pub epoch: u64,
    /// Frames accepted from this producer in the current generation.
    pub produced_frames: u64,
    /// Frames consumed in this generation, or zero for a stale consumer tuple.
    pub consumed_frames: u64,
    /// Accumulated consumer callback frames.
    pub consumer_ticks: u64,
    /// Whether the consumer is running in this generation.
    pub consumer_running: u32,
    /// Whether the source drives the graph clock.
    pub clock_master: u32,
    pub target_backlog_frames: u32,
    pub consumer_burst_frames: u32,
    pub starvation_episodes: u64,
    pub starvation_frames: u64,
    pub rebuffer_frames: u64,
    pub overwrite_skips: u64,
    pub insufficient_headroom: u32,
}

impl SourceTransport {
    /// Observes consumer state without waiting. Deferred observations preserve
    /// this producer's cursor and lease; only Revoked ends its ownership.
    #[must_use]
    pub fn observe_clock(&self) -> SourceClockRead {
        if !self.owns_stream() {
            return SourceClockRead::Revoked;
        }
        let clock = self.mapping.clock();
        let sequence = clock.consumer_sequence.load(Ordering::Acquire);
        let format = clock.format_word.load(Ordering::Acquire);
        let position = clock.consumer_position.load(Ordering::Acquire);
        let ticks = clock.consumer_ticks.load(Ordering::Acquire);
        let running = clock.consumer_running.load(Ordering::Acquire);
        let target = clock.target_backlog_frames.load(Ordering::Acquire);
        if sequence & 1 != 0
            || sequence != clock.consumer_sequence.load(Ordering::Acquire)
            || format != clock.format_word.load(Ordering::Acquire)
        {
            return if self.owns_stream() {
                SourceClockRead::Deferred
            } else {
                SourceClockRead::Revoked
            };
        }
        if !self.owns_stream() {
            return SourceClockRead::Revoked;
        }
        let current = position >> 48 == self.owned_epoch;
        SourceClockRead::Ready(SourceClockSnapshot {
            format_word: format,
            epoch: self.owned_epoch,
            produced_frames: self.next_frame,
            consumed_frames: if current { position & FRAME_MASK } else { 0 },
            consumer_ticks: ticks,
            consumer_running: if current { running } else { 0 },
            clock_master: clock.clock_master.load(Ordering::Acquire),
            target_backlog_frames: if current { target } else { 0 },
            consumer_burst_frames: clock.consumer_burst_frames.load(Ordering::Acquire),
            starvation_episodes: clock.starvation_episodes.load(Ordering::Acquire),
            starvation_frames: clock.starvation_frames.load(Ordering::Acquire),
            rebuffer_frames: clock.rebuffer_frames.load(Ordering::Acquire),
            overwrite_skips: clock.overwrite_skips.load(Ordering::Acquire),
            insufficient_headroom: clock.insufficient_headroom.load(Ordering::Acquire),
        })
    }

    /// How the consumer reads this epoch.
    #[must_use]
    pub fn consumer_timing(&self) -> ConsumerTimingRead {
        let clock = self.mapping.clock();
        let sequence = clock.consumer_sequence.load(Ordering::Acquire);
        let position = clock.consumer_position.load(Ordering::Acquire);
        let phase = clock.consumer_phase.load(Ordering::Acquire);
        let period = f64::from_bits(clock.consumer_period.load(Ordering::Acquire));
        if sequence & 1 != 0
            || sequence != clock.consumer_sequence.load(Ordering::Acquire)
            || position >> 48 != self.owned_epoch
        {
            ConsumerTimingRead::Unknown
        } else if period > 0.0 {
            ConsumerTimingRead::Timed(ConsumerTiming {
                period_ns: period,
                phase_ns: (phase != NO_PHASE).then_some(phase),
            })
        } else {
            ConsumerTimingRead::Sequential
        }
    }

    /// How the consumer's active plan uses this source.
    #[must_use]
    pub fn consumer_route(&self) -> ConsumerRoute {
        // Acquire pairs with the consumer's publication of its plan's route.
        ConsumerRoute::from_word(self.mapping.clock().consumer_route.load(Ordering::Acquire))
    }

    /// Compatibility projection for the C clock facade, which returns a bool.
    #[must_use]
    pub fn clock(&self) -> Option<SourceClockSnapshot> {
        match self.observe_clock() {
            SourceClockRead::Ready(clock) => Some(clock),
            SourceClockRead::Deferred | SourceClockRead::Revoked => None,
        }
    }

    pub fn reset(&mut self, format: u64, native_rate: u32) -> bool {
        self.begin_epoch(format, native_rate).is_some()
    }

    /// Acknowledges the requested format and returns the committed producer epoch.
    /// Busy claims or changed requests defer without requiring a consumer snapshot.
    pub fn begin_epoch(&mut self, format: u64, native_rate: u32) -> Option<u64> {
        if format == 0 || !(8000..=192_000).contains(&native_rate) {
            return None;
        }
        let clock = self.mapping.clock();
        let _claim = ProducerClaim::acquire(clock)?;
        if !self.owns_stream() || clock.format_word.load(Ordering::Acquire) != format {
            return None;
        }
        flush_source_faults(clock, &mut self.pending_source_faults);
        let previous_frames = clock.native_frames.load(Ordering::Acquire);
        let previous_rate = clock.native_sample_rate.load(Ordering::Acquire);
        clock.acknowledged_format.store(0, Ordering::Release);
        clock.native_running.store(0, Ordering::Release);
        clock.native_frames.store(0, Ordering::Release);
        clock
            .native_sample_rate
            .store(native_rate, Ordering::Release);
        // The ring's next epoch release publishes its initial zero credit and
        // native rate together. A reader acquiring that epoch cannot observe
        // the previous stream's frame count or rate with new running state.
        // SAFETY: the claim excludes registry replacement and all other producers.
        // Only this producer rearms; the consumer retains its independent cursor.
        let epoch = unsafe { mr_shared_audio_buffer_try_begin_epoch(self.mapping.buffer.as_ptr()) };
        if epoch == 0 {
            // Failed rearm leaves the ring invalid and unacknowledged. Preserve
            // its previous diagnostic rate/position while keeping activity stopped.
            clock
                .native_sample_rate
                .store(previous_rate, Ordering::Release);
            clock
                .native_frames
                .store(previous_frames, Ordering::Release);
            return None;
        }
        self.owned_epoch = epoch;
        self.next_frame = 0;
        clock.producer_burst_frames.store(0, Ordering::Release);
        clock.converter_burst_frames.store(0, Ordering::Release);
        clock.publication_drought_frames.store(0, Ordering::Release);
        clock.acknowledged_format.store(format, Ordering::Release);
        Some(epoch)
    }

    pub fn publish_measurements(&self, producer: u32, converter: u32, drought: u32) {
        let clock = self.mapping.clock();
        let Some(_claim) = ProducerClaim::acquire(clock) else {
            return;
        };
        if self.owns_stream() {
            clock
                .producer_burst_frames
                .fetch_max(producer, Ordering::Release);
            clock
                .converter_burst_frames
                .fetch_max(converter, Ordering::Release);
            clock
                .publication_drought_frames
                .fetch_max(drought, Ordering::Release);
        }
    }

    pub fn publish_native(&self, frames: u64, running: bool) {
        let clock = self.mapping.clock();
        let Some(_claim) = ProducerClaim::acquire(clock) else {
            return;
        };
        if self.owns_stream() {
            clock.native_frames.store(frames, Ordering::Release);
            clock
                .native_running
                .store(u32::from(running), Ordering::Release);
        }
    }

    /// Ends native activity while preserving its last committed frame credit.
    pub fn stop_native(&self) {
        let clock = self.mapping.clock();
        let Some(_claim) = ProducerClaim::acquire(clock) else {
            return;
        };
        if self.owns_stream() {
            clock.native_running.store(0, Ordering::Release);
        }
    }
}
