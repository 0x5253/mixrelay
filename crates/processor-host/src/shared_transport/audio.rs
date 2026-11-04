use std::sync::atomic::Ordering;

use super::SharedHeader;
use crate::AudioLayout;

use super::{Mapping, input_offset};

/// The engine callback's non-cloneable producer/consumer endpoint.
pub(crate) struct ParentAudio {
    mapping: Mapping,
    output_cache: Vec<f32>,
    output_tags: Vec<Option<u64>>,
    source_frame: u64,
    history_start: u64,
}

impl ParentAudio {
    pub(super) fn from_mapping(mapping: Mapping) -> Result<Self, String> {
        let output_samples = mapping.output_samples();
        let mut output_tags = mixrelay_audio_io::pcm_buffer::reserve(mapping.layout.block_frames)
            .map_err(|error| {
            format!("cannot allocate processor output coordinates: {error}")
        })?;
        output_tags.resize(mapping.layout.block_frames, None);
        let output_cache = mixrelay_audio_io::pcm_buffer::zeroed(output_samples)
            .map_err(|error| format!("cannot allocate processor output cache: {error}"))?;
        Ok(Self {
            mapping,
            output_cache,
            output_tags,
            source_frame: 0,
            history_start: 0,
        })
    }

    /// Invalidates queued, cached and in-flight audio without waiting for the worker.
    /// The monotonic frame clock is not rewound: old packets are acknowledged normally
    /// but cannot match any post-resume output range. Repeated suspension is O(1).
    pub(crate) fn discard_history(&mut self) {
        self.history_start = self.source_frame;
    }

    pub(crate) fn header(&self) -> &SharedHeader {
        self.mapping.header()
    }

    /// Consumes one new callback's input timeline, even if the occupied slot
    /// rejects its packet. Never retry that callback: doing so advances time
    /// twice. Invalid dimensions or sequences do not consume timeline frames.
    pub(crate) fn submit(
        &mut self,
        sequence: u64,
        input: &[f32],
        frames: usize,
        before_publish: impl FnOnce(),
    ) -> bool {
        let mapping = &self.mapping;
        let header = mapping.header();
        let Some(samples) = mapping.input_sample_count(frames) else {
            return false;
        };
        let Ok(frames_u32) = u32::try_from(frames) else {
            return false;
        };
        let frames_u64 = u64::from(frames_u32);
        let Some(next_source_frame) = self.source_frame.checked_add(frames_u64) else {
            return false;
        };
        let consumed_sequence = header.input_consumed.load(Ordering::Acquire);
        if frames == 0 || input.len() < samples || sequence == 0 || sequence == u64::MAX {
            return false;
        }
        let published_sequence = header.input_sequence.load(Ordering::Relaxed);
        if sequence <= published_sequence {
            return false;
        }
        self.source_frame = next_source_frame;
        if published_sequence != consumed_sequence {
            return false;
        }
        // SAFETY: this unique parent endpoint holds the input slot after acquiring the worker's
        // acknowledgment. The bounded borrow ends before the release publishes new input.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "Mapping admission validates header alignment; PCM offsets are multiples of f32 alignment."
        )]
        let destination = unsafe {
            std::slice::from_raw_parts_mut(
                mapping.raw.as_mut_ptr().add(input_offset()).cast::<f32>(),
                mapping.input_samples(),
            )
        };
        destination[..samples].copy_from_slice(&input[..samples]);
        header
            .input_first_frame
            .store(next_source_frame - frames_u64, Ordering::Relaxed);
        header.input_frames.store(frames_u32, Ordering::Relaxed);
        before_publish();
        header.input_sequence.store(sequence, Ordering::Release);
        // The worker sleeps on this word between blocks instead of polling it.
        mapping.wake.signal(&header.input_sequence);
        true
    }

    /// Harvests one returned packet and consumes this callback's output range.
    /// Call once before `submit`; `true` means the entire delayed range was
    /// available (including deterministic startup silence), not merely that a
    /// new packet arrived. Missing ranges are not retried or shifted into a later callback.
    pub(crate) fn receive(
        &mut self,
        output: &mut [f32],
        frames: usize,
        mut on_received: impl FnMut(u64),
    ) -> bool {
        let layout = self.mapping.layout;
        let Some(samples) = self.mapping.output_sample_count(frames) else {
            return false;
        };
        if frames == 0 || output.len() < samples {
            return false;
        }

        let sequence = {
            let header = self.mapping.header();
            header.output_sequence.load(Ordering::Acquire)
        };
        if sequence != 0
            && sequence
                != self
                    .mapping
                    .header()
                    .output_consumed
                    .load(Ordering::Relaxed)
        {
            self.harvest_output(sequence);
            on_received(sequence);
        }
        let Ok(block_frames_u64) = u64::try_from(layout.block_frames) else {
            return false;
        };
        let Ok(frames_u64) = u64::try_from(frames) else {
            return false;
        };
        // A prefix too large for usize is necessarily longer than this callback.
        let startup_frames = usize::try_from(
            block_frames_u64.saturating_sub(self.source_frame - self.history_start),
        )
        .map_or(frames, |prefix| prefix.min(frames));
        if startup_frames == frames {
            // This is the prepared IPC delay, not a worker failure. Returning
            // false would make the graph bypass current input immediately and
            // replay the same input when the delayed response arrives.
            output[..samples].fill(0.0);
            return true;
        }
        let request_start = self
            .source_frame
            .saturating_sub(block_frames_u64)
            .max(self.history_start);
        let Some(request_end) = request_start.checked_add(frames_u64 - startup_frames as u64)
        else {
            return false;
        };
        let mut ready = true;
        for frame in request_start..request_end {
            let slot = cache_slot(frame, layout.block_frames);
            if self.output_tags[slot] != Some(frame) {
                ready = false;
            }
        }
        if ready {
            // A callback can straddle the initial delay: retain its available suffix
            // instead of discarding that audio together with the silent prefix.
            output[..startup_frames * layout.outputs].fill(0.0);
            for (frame_offset, frame) in (request_start..request_end).enumerate() {
                let slot = cache_slot(frame, layout.block_frames);
                let source_start = slot * layout.outputs;
                let destination_start = (startup_frames + frame_offset) * layout.outputs;
                output[destination_start..destination_start + layout.outputs].copy_from_slice(
                    &self.output_cache[source_start..source_start + layout.outputs],
                );
            }
        }
        for frame in request_start..request_end {
            let slot = cache_slot(frame, layout.block_frames);
            self.output_tags[slot] = None;
        }
        ready
    }

    fn harvest_output(&mut self, sequence: u64) {
        let mapping = &self.mapping;
        let header = mapping.header();
        let first_frame = header.output_first_frame.load(Ordering::Relaxed);
        let raw_frames = header.output_frames.load(Ordering::Relaxed);
        let block_frames = mapping.layout.block_frames;
        let packet_frames_u64 = u64::from(raw_frames);
        // Invalid packet metadata is acknowledged without copying any audio.
        let packet_end = usize::try_from(raw_frames)
            .ok()
            .filter(|frames| (1..=block_frames).contains(frames))
            .and_then(|_| first_frame.checked_add(packet_frames_u64))
            .filter(|end| *end <= self.source_frame);
        if let Some(packet_end) = packet_end {
            let window_start = self.source_frame.saturating_sub(block_frames as u64);
            let copy_start = first_frame.max(window_start).max(self.history_start);
            // Both spans lie inside the validated packet, whose length fits usize.
            if copy_start < packet_end
                && let (Ok(frame_offset), Ok(copy_frames)) = (
                    usize::try_from(copy_start - first_frame),
                    usize::try_from(packet_end - copy_start),
                )
            {
                // SAFETY: output metadata is validated against the fixed block and source region.
                // The worker publishes the validated packet's interleaved samples before the
                // release sequence, so this borrow is within the mapped output region.
                #[expect(
                    clippy::cast_ptr_alignment,
                    reason = "Mapping admission validates header alignment; PCM offsets are multiples of f32 alignment."
                )]
                let source = unsafe {
                    std::slice::from_raw_parts(
                        mapping
                            .raw
                            .as_ptr()
                            .add(mapping.output_offset())
                            .cast::<f32>(),
                        mapping.output_samples(),
                    )
                };
                for offset in 0..copy_frames {
                    let frame = copy_start + offset as u64;
                    let slot = cache_slot(frame, block_frames);
                    let source_start = (frame_offset + offset) * mapping.layout.outputs;
                    let destination_start = slot * mapping.layout.outputs;
                    let destination_end = destination_start + mapping.layout.outputs;
                    self.output_cache[destination_start..destination_end].copy_from_slice(
                        &source[source_start..source_start + mapping.layout.outputs],
                    );
                    self.output_tags[slot] = Some(frame);
                }
            }
        }
        header.output_consumed.store(sequence, Ordering::Release);
        // A worker whose output slot was full sleeps on this word.
        mapping.wake.signal(&header.output_consumed);
    }
}

/// Maps a source frame to its output-cache slot.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the remainder is below `block_frames`, which is a usize"
)]
fn cache_slot(frame: u64, block_frames: usize) -> usize {
    (frame % block_frames as u64) as usize
}

/// The worker's single DSP endpoint retains its captured sequence while processing privately.
pub(crate) struct WorkerAudio {
    mapping: Mapping,
    output_packet: Option<OutputPacket>,
}

/// A captured input whose frame and output sample counts were validated on receipt.
#[derive(Clone, Copy)]
struct OutputPacket {
    sequence: u64,
    first_frame: u64,
    frames: u32,
    output_samples: usize,
}

impl WorkerAudio {
    pub(super) const fn from_mapping(mapping: Mapping) -> Self {
        Self {
            mapping,
            output_packet: None,
        }
    }

    pub(crate) fn header(&self) -> &SharedHeader {
        self.mapping.header()
    }
    pub(crate) fn layout(&self) -> AudioLayout {
        self.mapping.layout
    }

    /// The words the parent advances to unblock this worker. Snapshot them, pick the one
    /// to wait on with [`Self::blocking_word`], and only then check
    /// [`Self::input_pending`].
    pub(crate) fn wake_words(&self) -> (u64, u64) {
        let header = self.header();
        (
            header.input_sequence.load(Ordering::Acquire),
            header.output_consumed.load(Ordering::Acquire),
        )
    }

    /// Why no input is pending: a published output the parent has not consumed, or no
    /// new input. Returns that word and its snapshot from [`Self::wake_words`], so a
    /// change made after the snapshot ends the wait at once instead of being missed.
    pub(crate) fn blocking_word(
        &self,
        (input, consumed): (u64, u64),
    ) -> (&std::sync::atomic::AtomicU64, u64) {
        let header = self.header();
        if header.output_sequence.load(Ordering::Acquire)
            == header.output_consumed.load(Ordering::Acquire)
        {
            (&header.input_sequence, input)
        } else {
            (&header.output_consumed, consumed)
        }
    }

    pub(crate) fn input_pending(&self) -> bool {
        let header = self.header();
        let sequence = header.input_sequence.load(Ordering::Acquire);
        self.output_packet.is_none()
            && sequence != 0
            && sequence != header.input_consumed.load(Ordering::Relaxed)
            && header.output_sequence.load(Ordering::Acquire)
                == header.output_consumed.load(Ordering::Acquire)
    }

    pub(crate) fn receive(&mut self, copy: impl FnOnce(&[f32])) -> Option<usize> {
        if !self.input_pending() {
            return None;
        }
        let mapping = &self.mapping;
        let header = mapping.header();
        let sequence = header.input_sequence.load(Ordering::Acquire);
        let first_frame = header.input_first_frame.load(Ordering::Relaxed);
        let raw_frames = header.input_frames.load(Ordering::Relaxed);
        let counts = usize::try_from(raw_frames)
            .ok()
            .filter(|frames| {
                *frames != 0
                    && *frames <= mapping.layout.block_frames
                    && first_frame.checked_add(u64::from(raw_frames)).is_some()
            })
            .and_then(|frames| {
                Some((
                    frames,
                    mapping.input_sample_count(frames)?,
                    mapping.output_sample_count(frames)?,
                ))
            });
        let Some((frames, samples, output_samples)) = counts else {
            header.input_consumed.store(sequence, Ordering::Release);
            return None;
        };
        // SAFETY: input_pending acquired the parent's publication and reserved free output.
        // This sole worker cannot consume another input before publishing the captured output.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "Mapping admission validates header alignment; PCM offsets are multiples of f32 alignment."
        )]
        let source = unsafe {
            std::slice::from_raw_parts(
                mapping.raw.as_ptr().add(input_offset()).cast::<f32>(),
                samples,
            )
        };
        copy(source);
        self.output_packet = Some(OutputPacket {
            sequence,
            first_frame,
            frames: raw_frames,
            output_samples,
        });
        header.input_consumed.store(sequence, Ordering::Release);
        Some(frames)
    }

    pub(crate) fn publish(&mut self, copy: impl FnOnce(&mut [f32])) -> bool {
        let Some(packet) = self.output_packet else {
            return false;
        };
        let mapping = &self.mapping;
        // SAFETY: receive reserved this output slot after acquiring the parent's acknowledgment.
        // The unique worker holds it across private DSP; the slice ends before publication.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "Mapping admission validates header alignment; PCM offsets are multiples of f32 alignment."
        )]
        let destination = unsafe {
            std::slice::from_raw_parts_mut(
                mapping
                    .raw
                    .as_mut_ptr()
                    .add(mapping.output_offset())
                    .cast::<f32>(),
                packet.output_samples,
            )
        };
        copy(destination);
        mapping
            .header()
            .output_first_frame
            .store(packet.first_frame, Ordering::Relaxed);
        mapping
            .header()
            .output_frames
            .store(packet.frames, Ordering::Relaxed);
        mapping
            .header()
            .output_sequence
            .store(packet.sequence, Ordering::Release);
        self.output_packet = None;
        true
    }
}

/// Worker-side observations and acknowledgements for tests that run without a worker.
#[cfg(test)]
mod worker_double {
    use std::sync::atomic::Ordering;

    use super::ParentAudio;

    impl ParentAudio {
        /// Output samples the worker has consumed.
        pub(crate) fn acknowledged_output(&self) -> u64 {
            self.header().output_consumed.load(Ordering::Acquire)
        }

        /// Acknowledges the current block as the worker does, without a worker.
        #[cfg(not(debug_assertions))]
        pub(crate) fn acknowledge_benchmark_block(&self) {
            let header = self.header();
            let sequence = header.input_sequence.load(Ordering::Acquire);
            header.input_consumed.store(sequence, Ordering::Release);
            let block_frames = self.mapping.layout.block_frames;
            let first_frame = self
                .source_frame
                .saturating_sub(u64::try_from(block_frames).expect("u32 fits u64"));
            header
                .output_first_frame
                .store(first_frame, Ordering::Relaxed);
            header.output_frames.store(
                u32::try_from(block_frames).expect("mapping admission validates block size"),
                Ordering::Relaxed,
            );
            header.output_sequence.store(sequence, Ordering::Release);
        }
    }
}
