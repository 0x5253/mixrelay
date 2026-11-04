//! Graph-side source observation and PCM reads. Everything here may run on an
//! audio callback: it pins, claims and reads atomics, and never waits or allocates.
use super::{
    SourceClock, SourcePcmProgress, SourceRegistry, content_latency,
    identity::{MAX_LEASE, stream_epoch},
    latency::transition,
    timeline::ReadWindow,
    timestamps::{self, ReadMiss},
};
use crate::{
    application_clock::mr_application_clock_request_rate,
    shared_audio_buffer::mr_shared_audio_buffer_stats,
};
use std::sync::atomic::Ordering;

impl SourceRegistry {
    /// Observes the current producer lease and acknowledged native clock.
    #[must_use]
    pub fn clock(&self, token: u64) -> Option<SourceClock> {
        self.pcm_progress(token).map(|progress| progress.clock)
    }

    /// Observes a source clock and its retained graph-frame extent under one pin.
    #[must_use]
    pub fn pcm_progress(&self, token: u64) -> Option<SourcePcmProgress> {
        let read = self.read(token);
        let source = read.source()?;
        let clock = source.mapping.clock();
        let format = clock.format_word.load(Ordering::Acquire);
        let acknowledged = clock.acknowledged_format.load(Ordering::Acquire);
        let lease = clock.producer_lease.load(Ordering::Acquire);
        // SAFETY: the read pin retains the validated buffer through this snapshot.
        let pcm = unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) };
        let epoch = pcm.epoch;
        let frames = clock.native_frames.load(Ordering::Acquire);
        let rate = clock.native_sample_rate.load(Ordering::Acquire);
        let running = clock.native_running.load(Ordering::Acquire);
        let consumer = clock.consumer_position.load(Ordering::Acquire);
        let consumer_running = clock.consumer_running.load(Ordering::Acquire) != 0;
        // The second loads prove the fields above belong to one unchanged stream.
        if format == 0
            || format != acknowledged
            || epoch == 0
            || lease == 0
            || lease > MAX_LEASE
            || format != clock.format_word.load(Ordering::Acquire)
            || acknowledged != clock.acknowledged_format.load(Ordering::Acquire)
            || lease != clock.producer_lease.load(Ordering::Acquire)
            || epoch
                != unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }.epoch
        {
            return None;
        }
        Some(SourcePcmProgress {
            clock: SourceClock {
                epoch: stream_epoch(lease, epoch),
                native_frames: frames,
                native_rate: rate,
                graph_rate: u32::try_from(format & u64::from(u32::MAX)).ok()?,
                running,
                synchronized: u32::from(
                    running != 0 && consumer_running && consumer >> 48 == epoch,
                ),
            },
            published_frames: pcm.published_through_frame,
            capacity_frames: source.capacity,
        })
    }

    /// Reads one complete graph block under the source's single-consumer claim.
    pub fn read_audio(&self, token: u64, samples: &mut [f32], frames: u32, rate: u32) -> u32 {
        self.read_audio_time(
            token,
            samples,
            frames,
            content_latency::ReadTiming {
                rate,
                ..Default::default()
            },
        )
    }

    /// Reads PCM at an optional acquisition cursor and measures its age at the graph label.
    pub fn read_audio_time(
        &self,
        token: u64,
        samples: &mut [f32],
        frames: u32,
        timing: content_latency::ReadTiming,
    ) -> u32 {
        self.read_audio_timed(token, samples, frames, timing)
            .unwrap_or(0)
    }

    /// Reads one complete block like `read_audio_time`, reporting why none was read.
    ///
    /// # Errors
    /// Returns the [`ReadMiss`] reason; `samples` is then silent and the cursor not advanced.
    pub fn read_audio_timed(
        &self,
        token: u64,
        samples: &mut [f32],
        frames: u32,
        timing: content_latency::ReadTiming,
    ) -> Result<u32, ReadMiss> {
        let content_latency::ReadTiming {
            rate,
            first_nanos: requested_time,
            host_ticks,
        } = timing;
        samples.fill(0.0);
        let read = self.read(token);
        let source = read
            .source()
            .filter(|source| {
                frames != 0 && samples.len() == frames as usize * source.channels as usize
            })
            .ok_or(ReadMiss::Unavailable)?;
        let _claim = source.claim_consumer().ok_or(ReadMiss::Unavailable)?;
        let mut attempt = content_latency::Attempt::new(&source.measurements);
        // SAFETY: this nonblocking consumer claim is the only access to the
        // timeline; no control method modifies it, including replacement.
        let timeline = unsafe { &mut *source.timeline.get() };
        let clock = source.mapping.clock();
        let stats = unsafe { mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) };
        let format =
            unsafe { mr_application_clock_request_rate(source.mapping.clock.as_ptr(), rate) };
        let lease = clock.producer_lease.load(Ordering::Acquire);
        let valid = lease != 0
            && format != 0
            && stats.epoch != 0
            && clock.acknowledged_format.load(Ordering::Acquire) == format;
        if !valid {
            // The reader's clock survives an unavailable source; its next read restarts
            // the measured run, so it is never taken for a sequential reader meanwhile.
            timeline.restart();
            timeline.publish(source, stats.epoch, frames, false);
            return Err(ReadMiss::Unavailable);
        }
        let observation = timeline.observe(source, stats, frames, (rate, self.clock.now()));
        let (mut policy, ready) = transition(timeline.policy, observation);
        if !ready {
            if source.read_is_current(lease, format, stats.epoch) {
                timeline.policy = policy;
            }
            timeline.publish(source, stats.epoch, frames, false);
            return Err(ReadMiss::NotReady);
        }
        if let Err(miss) = timestamps::select(
            source,
            stats,
            timeline.policy,
            &mut policy,
            (rate, frames, requested_time),
        ) {
            if source.read_is_current(lease, format, stats.epoch) {
                // A timestamp miss still observes the scheduling burst. Retain its
                // reset/growth without advancing the unread PCM cursor.
                timeline.policy = policy;
            }
            timeline.publish(source, stats.epoch, frames, false);
            return Err(miss);
        }
        let copied = timeline.copy_audio(
            source,
            samples,
            policy,
            ReadWindow {
                observation,
                lease,
                format,
                graph_ns: host_ticks.map(|host| self.timebase.nanoseconds(host)),
            },
        );
        attempt.completed = copied != 0;
        if copied == 0 {
            return Err(ReadMiss::Incomplete);
        }
        Ok(copied)
    }

    /// Takes normalized-sample faults once for the current source lease.
    pub fn take_source_faults(&self, token: u64) -> u64 {
        let read = self.read(token);
        read.source().map_or(0, |source| {
            source.mapping.clock().source_sample_faults.take()
        })
    }
}
