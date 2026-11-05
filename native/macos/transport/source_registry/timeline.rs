//! Consumer-owned read cursor and its publication to the producer's clock.
//! Only the holder of a source's consumer claim touches a timeline.
use super::{
    content_latency,
    latency::{Latency, Observation, transition},
    source::Source,
};
use crate::{
    application_clock::{ApplicationClock, ConsumerPublication},
    shared_audio_buffer::{SharedAudioBufferStats, mr_shared_audio_buffer_read},
};
use mixrelay_time::Instant;
use std::sync::atomic::Ordering;

/// What one read observed before copying, rechecked after the copy.
#[derive(Clone, Copy)]
pub(super) struct ReadWindow {
    pub(super) observation: Observation,
    pub(super) lease: u64,
    pub(super) format: u64,
    pub(super) graph_ns: Option<u64>,
}

#[derive(Default)]
pub(super) struct Timeline {
    pub(super) policy: Latency,
    previous_read: Option<Instant>,
    previous_frames: u32,
}

impl Timeline {
    /// Forgets the measured run after the source became unavailable.
    pub(super) fn restart(&mut self) {
        self.policy.started = false;
        self.previous_read = None;
        self.policy.next = 0;
    }

    /// Makes the next read start a new scheduling burst, as after a long pause.
    #[cfg(test)]
    pub(super) fn separate_next_read(&mut self) {
        self.previous_read = None;
    }

    pub(super) fn observe(
        &mut self,
        source: &Source,
        stats: SharedAudioBufferStats,
        frames: u32,
        timing: (u32, Instant),
    ) -> Observation {
        let clock = source.mapping.clock();
        let (rate, now) = timing;
        // Full scheduling bursts may comprise many graph slices. Accumulate
        // adjacent calls, independently of concurrent producer publications.
        let adjacent = self
            .previous_read
            .filter(|previous| now >= *previous)
            .map(|previous| now.duration_since(previous))
            .is_some_and(|elapsed| {
                elapsed.as_nanos() * u128::from(rate) * 2
                    < u128::from(self.previous_frames) * 1_000_000_000
            });
        self.previous_read = Some(now);
        self.previous_frames = frames;
        Observation {
            epoch: stats.epoch,
            published: stats.published_through_frame,
            frames,
            capacity: source.capacity,
            producer_burst: clock.producer_burst_frames.load(Ordering::Acquire),
            converter_burst: clock.converter_burst_frames.load(Ordering::Acquire),
            drought: clock.publication_drought_frames.load(Ordering::Acquire),
            running: clock.native_running.load(Ordering::Acquire) != 0,
            adjacent,
            copy_missing: 0,
        }
    }

    /// Copies the selected window; returns the copied frames, or zero with
    /// silent `samples` when the copy was short or the stream changed meanwhile.
    pub(super) fn copy_audio(
        &mut self,
        source: &Source,
        samples: &mut [f32],
        policy: Latency,
        window: ReadWindow,
    ) -> u32 {
        let ReadWindow {
            observation,
            lease,
            format,
            graph_ns,
        } = window;
        let frames = observation.frames;
        let age = graph_ns.and_then(|label| {
            content_latency::capture_age(source, observation.epoch, policy.next, frames, label)
        });
        // SAFETY: the caller holds this source's consumer claim and mapping pin;
        // samples is the complete exclusive destination for the validated frame count.
        let available = unsafe {
            mr_shared_audio_buffer_read(
                source.mapping.buffer.as_ptr(),
                policy.next,
                samples.as_mut_ptr(),
                frames,
            )
        };
        let complete =
            source.read_is_current(lease, format, observation.epoch) && available == frames;
        if complete {
            self.policy = policy.consumed(frames);
            source.measurements.age.record(age, u64::from(frames));
        } else if source.read_is_current(lease, format, observation.epoch) {
            self.policy = transition(
                policy,
                Observation {
                    copy_missing: frames - available,
                    ..observation
                },
            )
            .0;
        }
        self.publish(source, observation.epoch, frames, complete);
        if complete {
            available
        } else {
            samples.fill(0.0);
            0
        }
    }

    pub(super) fn publish(&self, source: &Source, epoch: u64, frames: u32, running: bool) {
        self.publish_telemetry(source.mapping.clock());
        source
            .mapping
            .clock()
            .publish_consumer(ConsumerPublication {
                epoch,
                next_frame: self.policy.next,
                frames,
                running,
                target_backlog_frames: self.policy.target,
                phase_ns: self.policy.timing.phase_ns(),
                period_ns: self.policy.timing.period_ns(),
            });
    }

    fn publish_telemetry(&self, clock: &ApplicationClock) {
        let policy = self.policy;
        clock
            .starvation_episodes
            .store(policy.starvation_episodes, Ordering::Release);
        clock
            .starvation_frames
            .store(policy.starvation_frames, Ordering::Release);
        clock
            .rebuffer_frames
            .store(policy.rebuffer_frames, Ordering::Release);
        clock
            .overwrite_skips
            .store(policy.overwrite_skips, Ordering::Release);
        clock
            .consumer_burst_frames
            .store(policy.consumer_burst, Ordering::Release);
        clock
            .insufficient_headroom
            .store(u32::from(policy.insufficient_headroom), Ordering::Release);
    }
}
