use std::mem::size_of;

use thiserror::Error;

const MAX_CHANNELS: usize = 64;
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

/// Errors returned while preparing bounded per-route delay history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(super) enum RouteDelayError {
    /// The interleaved channel width is outside the graph's supported range.
    #[error("channel count {channels} is outside the range 1..=64")]
    InvalidChannels { channels: usize },
    /// The requested frame and channel dimensions overflow `usize`.
    #[error("delay history size overflows addressable memory")]
    CapacityOverflow,
    /// The requested history exceeds this delay line's memory bound.
    #[error("history needs {requested_bytes} bytes; maximum is {maximum_bytes} bytes")]
    HistoryTooLarge {
        requested_bytes: usize,
        maximum_bytes: usize,
    },
    /// The allocator could not reserve the requested history before rendering.
    #[error("could not reserve {samples} delay history samples")]
    AllocationFailed { samples: usize },
}

/// Preallocated interleaved PCM history for one graph route.
///
/// The line retains at most 16 MiB of logical `f32` history. Its callback methods only access
/// storage prepared by [`RouteDelayLine::new`].
#[derive(Debug)]
pub(super) struct RouteDelayLine {
    channels: usize,
    delay_frames: usize,
    history: Vec<f32>,
    cursor_frame: usize,
    valid_frames: usize,
}

impl RouteDelayLine {
    /// Prepares a delay line with a fixed interleaved channel width and frame delay.
    ///
    /// A zero-frame delay allocates no history and copies input directly during processing.
    /// Nonzero history is limited to 16 MiB per line.
    ///
    /// # Arguments
    ///
    /// * `channels` - Interleaved channel count, from 1 through 64.
    /// * `delay_frames` - Number of input frames to retain before producing delayed PCM.
    ///
    /// # Returns
    ///
    /// A prepared delay line with all required history storage reserved.
    ///
    /// # Errors
    ///
    /// Returns [`RouteDelayError::InvalidChannels`] for channel counts outside 1 through 64,
    /// [`RouteDelayError::CapacityOverflow`] if the sample count cannot be represented,
    /// [`RouteDelayError::HistoryTooLarge`] if history exceeds the per-line bound, or
    /// [`RouteDelayError::AllocationFailed`] if the history cannot be reserved.
    pub(super) fn new(channels: usize, delay_frames: usize) -> Result<Self, RouteDelayError> {
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(RouteDelayError::InvalidChannels { channels });
        }

        let samples = delay_frames
            .checked_mul(channels)
            .ok_or(RouteDelayError::CapacityOverflow)?;
        let requested_bytes = samples
            .checked_mul(size_of::<f32>())
            .ok_or(RouteDelayError::CapacityOverflow)?;
        if requested_bytes > MAX_HISTORY_BYTES {
            return Err(RouteDelayError::HistoryTooLarge {
                requested_bytes,
                maximum_bytes: MAX_HISTORY_BYTES,
            });
        }

        let mut history = Vec::new();
        if samples > 0 {
            history
                .try_reserve_exact(samples)
                .map_err(|_| RouteDelayError::AllocationFailed { samples })?;
            history.resize(samples, 0.0);
        }

        Ok(Self {
            channels,
            delay_frames,
            history,
            cursor_frame: 0,
            valid_frames: 0,
        })
    }

    pub(super) fn primed(&self) -> bool {
        self.valid_frames == self.delay_frames
    }

    /// Resets transport history without clearing the backing PCM allocation.
    ///
    /// The next `delay_frames` of output are silence, so pre-reset samples cannot reappear.
    pub(super) fn reset(&mut self) {
        self.cursor_frame = 0;
        self.valid_frames = 0;
    }

    /// Delays `frames` of interleaved input into the corresponding output span.
    ///
    /// Input and output use the channel width supplied to [`RouteDelayLine::new`]. Only the first
    /// `frames * channels` samples are accessed; any longer suffix is left untouched. Before the
    /// history is filled, output is silence. A zero-frame call succeeds without changing state.
    ///
    /// # Arguments
    ///
    /// * `input` - Interleaved input samples for the requested frames.
    /// * `output` - Writable interleaved output span.
    /// * `frames` - Number of frames to process.
    ///
    /// # Returns
    ///
    /// `true` when the requested spans fit and processing completed, or `false` when either span
    /// is undersized or the sample count overflows. A rejected call leaves the line and output
    /// unchanged.
    pub(super) fn advance(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        let Some(sample_count) = frames.checked_mul(self.channels) else {
            return false;
        };
        if input.len() < sample_count || output.len() < sample_count {
            return false;
        }
        if frames == 0 {
            return true;
        }
        if self.delay_frames == 0 {
            output[..sample_count].copy_from_slice(&input[..sample_count]);
            return true;
        }

        let mut completed_frames = 0;
        while completed_frames < frames {
            let ring_frames_remaining = self.delay_frames - self.cursor_frame;
            let chunk_frames = (frames - completed_frames).min(ring_frames_remaining);
            let chunk_samples = chunk_frames * self.channels;
            let history_start = self.cursor_frame * self.channels;
            let pcm_start = completed_frames * self.channels;

            let warmup_frames = chunk_frames.min(self.delay_frames - self.valid_frames);
            let warmup_samples = warmup_frames * self.channels;
            if warmup_samples > 0 {
                let history_end = history_start + warmup_samples;
                let pcm_end = pcm_start + warmup_samples;
                self.history[history_start..history_end]
                    .copy_from_slice(&input[pcm_start..pcm_end]);
                output[pcm_start..pcm_end].fill(0.0);
                self.valid_frames += warmup_frames;
            }

            let history_hot_start = history_start + warmup_samples;
            let pcm_hot_start = pcm_start + warmup_samples;
            let hot_samples = chunk_samples - warmup_samples;
            if hot_samples > 0 {
                let history_hot_end = history_hot_start + hot_samples;
                let pcm_hot_end = pcm_hot_start + hot_samples;
                output[pcm_hot_start..pcm_hot_end]
                    .copy_from_slice(&self.history[history_hot_start..history_hot_end]);
                self.history[history_hot_start..history_hot_end]
                    .copy_from_slice(&input[pcm_hot_start..pcm_hot_end]);
            }

            self.cursor_frame += chunk_frames;
            if self.cursor_frame == self.delay_frames {
                self.cursor_frame = 0;
            }
            completed_frames += chunk_frames;
        }

        true
    }

    /// Returns the allocated number of `f32` samples held by this line.
    ///
    /// This is the prepared history size used to account for graph delay memory.
    #[must_use]
    pub(super) fn history_samples(&self) -> usize {
        self.history.capacity()
    }
}

#[cfg(test)]
mod tests;
