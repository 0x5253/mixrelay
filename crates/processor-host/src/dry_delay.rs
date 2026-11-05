//! Preallocated fixed-frame dry history for a hosted worker's late-response fallback.
//!
//! The delay models the known IPC block latency only. It does not include plug-in intrinsic
//! latency and is not a graph-level parallel-path compensation mechanism.

use mixrelay_audio_graph::{ProviderPorts, ProviderPortsError};

pub(crate) mod channel_map;
mod dimensions;
#[cfg(test)]
mod tests;

/// Failure to configure or allocate a worker-owned dry-delay ring.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DryDelayError {
    /// Channel, frame, stride, or history dimensions exceeded a supported bound.
    #[error("{0}")]
    InvalidLayout(&'static str),
    /// The bounded history could not be reserved from the process allocator.
    #[error("dry-delay history allocation failed: {0}")]
    Allocation(#[from] std::collections::TryReserveError),
    /// The saved/native port description is malformed or exceeds its bounds.
    #[error("invalid dry-delay provider ports: {0}")]
    ProviderPorts(#[from] ProviderPortsError),
}

/// Fixed-frame, channel-preserving history owned by the worker.
///
/// The ring starts with zeroes and stores only input channels needed by its output map. All
/// storage and any explicit map are allocated during construction; [`DryDelay::advance`] never
/// allocates, locks, or calls a plug-in API.
pub(crate) struct DryDelay {
    input_channels: usize,
    output_channels: usize,
    stored_channels: usize,
    delay_frames: usize,
    cursor: usize,
    valid_history_frames: usize,
    /// `None` is the canonical flat prefix map; `Some` maps each output channel to a selected
    /// input channel or silence. Explicit maps are bounded by validated provider port limits.
    mapping: Option<Vec<Option<usize>>>,
    /// Frame-interleaved ring; each slot holds `stored_channels` samples.
    history: Vec<f32>,
}

impl DryDelay {
    /// Create a delay whose dry channel routing follows the provider's main-bus topology.
    ///
    /// Each main output bus maps to the main input bus at the same main-bus ordinal, copying
    /// matching channel positions. Sidechain and auxiliary buses are never connected to dry
    /// outputs; unmatched main output channels and every non-main output are silent.
    ///
    /// # Errors
    /// Rejects invalid provider geometry or dimensions and reports failed bounded allocations.
    pub(crate) fn for_ports(
        ports: &ProviderPorts,
        delay_frames: usize,
    ) -> Result<Self, DryDelayError> {
        let mapping = channel_map::main_bus_map(ports)?;
        let input_channels = ports
            .inputs
            .iter()
            .map(|bus| usize::from(bus.channels))
            .sum();
        let output_channels = ports
            .outputs
            .iter()
            .map(|bus| usize::from(bus.channels))
            .sum();
        Self::with_mapping(input_channels, output_channels, delay_frames, Some(mapping))
    }

    /// Build a flat-prefix delay: each output copies the input at the same position, and
    /// outputs past the shared leading channels are silent. Production routing uses
    /// [`Self::for_ports`] so dry routing follows the declared main-bus topology.
    #[cfg(test)]
    pub(crate) fn new(
        input_channels: usize,
        output_channels: usize,
        delay_frames: usize,
    ) -> Result<Self, DryDelayError> {
        Self::with_mapping(input_channels, output_channels, delay_frames, None)
    }

    fn with_mapping(
        input_channels: usize,
        output_channels: usize,
        delay_frames: usize,
        mapping: Option<Vec<Option<usize>>>,
    ) -> Result<Self, DryDelayError> {
        let (flat_channels, _) =
            dimensions::checked_sizes(input_channels, output_channels, delay_frames)
                .map_err(DryDelayError::InvalidLayout)?;
        if let Some(mapping) = &mapping {
            validate_mapping(mapping, input_channels, output_channels)?;
        }
        // Normalize an equivalent flat-prefix map to preserve the direct-copy fast path.
        let mapping = mapping.filter(|mapping| !is_flat_prefix(mapping, flat_channels));
        let stored_channels = mapping
            .as_deref()
            .map_or(flat_channels, highest_mapped_channel_count);
        Ok(Self {
            input_channels,
            output_channels,
            stored_channels,
            delay_frames,
            cursor: 0,
            valid_history_frames: 0,
            mapping,
            history: zeroed_history(stored_channels, delay_frames)?,
        })
    }

    /// Reset delay age without clearing or reallocating the physical history storage.
    ///
    /// The next `delay_frames` inputs prime the ring; delayed output remains silent until the
    /// ring contains only samples written since this reset. This operation is O(1).
    pub(crate) fn reset(&mut self) {
        self.cursor = 0;
        self.valid_history_frames = 0;
    }

    /// Advance the ring by `frames`, optionally writing the delayed dry signal.
    ///
    /// `input` and `output` use frame-interleaved layout with exactly `frames` frame groups;
    /// longer slices are accepted. `None` advances history without producing output.
    /// When output is present, matching channels receive the delayed input and unmatched outputs
    /// are zeroed. Returns `false` without changing state or output for zero/oversized frame
    /// counts or undersized slices.
    pub(crate) fn advance(
        &mut self,
        input: &[f32],
        output: Option<&mut [f32]>,
        frames: usize,
    ) -> bool {
        if !self.accepts(input, output.as_deref(), frames) {
            return false;
        }
        match output {
            Some(output) if self.is_direct_copy() => self.advance_direct(input, output, frames),
            None if self.is_direct_copy() => self.record_direct(input, frames),
            output => self.advance_mapped(input, output, frames),
        }
        true
    }

    fn accepts(&self, input: &[f32], output: Option<&[f32]>, frames: usize) -> bool {
        if frames == 0 || frames > self.delay_frames {
            return false;
        }
        let Some(input_samples) = self.input_channels.checked_mul(frames) else {
            return false;
        };
        let Some(output_samples) = self.output_channels.checked_mul(frames) else {
            return false;
        };
        input.len() >= input_samples && output.is_none_or(|slice| slice.len() >= output_samples)
    }

    /// Input, history and output frames share one layout, so whole blocks copy at once.
    fn is_direct_copy(&self) -> bool {
        self.mapping.is_none()
            && self.input_channels == self.stored_channels
            && self.output_channels == self.stored_channels
    }

    fn advance_direct(&mut self, input: &[f32], output: &mut [f32], frames: usize) {
        let channels = self.stored_channels;
        let total_samples = frames * channels;
        let silent_frames = (self.delay_frames - self.valid_history_frames).min(frames);
        let silent_samples = silent_frames * channels;
        output[..silent_samples].fill(0.0);
        let delayed_output = &mut output[silent_samples..total_samples];
        if !delayed_output.is_empty() {
            let first_delayed_cursor =
                advance_cursor(self.cursor, silent_frames, self.delay_frames);
            self.copy_history_to_output(first_delayed_cursor, delayed_output);
        }
        self.record_direct(input, frames);
    }

    fn record_direct(&mut self, input: &[f32], frames: usize) {
        self.copy_input_to_history(self.cursor, &input[..frames * self.stored_channels]);
        self.advance_position(frames);
    }

    /// Frame-by-frame path for channel maps and differing input/output widths.
    fn advance_mapped(&mut self, input: &[f32], mut output: Option<&mut [f32]>, frames: usize) {
        for frame in 0..frames {
            let history_start = self.cursor * self.stored_channels;
            if let Some(output) = output.as_deref_mut() {
                let output_start = frame * self.output_channels;
                self.write_delayed_frame(
                    &mut output[output_start..output_start + self.output_channels],
                    history_start,
                );
            }
            if self.stored_channels > 0 {
                let input_start = frame * self.input_channels;
                self.history[history_start..history_start + self.stored_channels]
                    .copy_from_slice(&input[input_start..input_start + self.stored_channels]);
            }
            self.advance_position(1);
        }
    }

    /// Writes one delayed frame, or silence while the ring still holds pre-reset samples.
    fn write_delayed_frame(&self, output: &mut [f32], history_start: usize) {
        let history_is_valid = self.valid_history_frames == self.delay_frames;
        for (output_channel, sample) in output.iter_mut().enumerate() {
            *sample = match self.source_channel(output_channel) {
                Some(input_channel) if history_is_valid => {
                    self.history[history_start + input_channel]
                }
                _ => 0.0,
            };
        }
    }

    fn advance_position(&mut self, frames: usize) {
        self.cursor = advance_cursor(self.cursor, frames, self.delay_frames);
        let unfilled_frames = self.delay_frames - self.valid_history_frames;
        self.valid_history_frames += frames.min(unfilled_frames);
    }

    fn copy_history_to_output(&self, cursor: usize, output: &mut [f32]) {
        let start = cursor * self.stored_channels;
        let first_samples = (self.history.len() - start).min(output.len());
        output[..first_samples].copy_from_slice(&self.history[start..start + first_samples]);
        let remaining = output.len() - first_samples;
        if remaining > 0 {
            output[first_samples..].copy_from_slice(&self.history[..remaining]);
        }
    }

    fn copy_input_to_history(&mut self, cursor: usize, input: &[f32]) {
        let start = cursor * self.stored_channels;
        let first_samples = (self.history.len() - start).min(input.len());
        self.history[start..start + first_samples].copy_from_slice(&input[..first_samples]);
        let remaining = input.len() - first_samples;
        if remaining > 0 {
            self.history[..remaining].copy_from_slice(&input[first_samples..]);
        }
    }

    fn source_channel(&self, output_channel: usize) -> Option<usize> {
        self.mapping.as_ref().map_or(
            (output_channel < self.stored_channels).then_some(output_channel),
            |mapping| mapping[output_channel],
        )
    }
}

fn validate_mapping(
    mapping: &[Option<usize>],
    input_channels: usize,
    output_channels: usize,
) -> Result<(), DryDelayError> {
    if mapping.len() != output_channels
        || mapping
            .iter()
            .flatten()
            .any(|channel| *channel >= input_channels)
    {
        return Err(DryDelayError::InvalidLayout(
            "dry-delay channel map does not match its channel dimensions",
        ));
    }
    Ok(())
}

fn is_flat_prefix(mapping: &[Option<usize>], flat_channels: usize) -> bool {
    mapping
        .iter()
        .enumerate()
        .all(|(output, input)| *input == (output < flat_channels).then_some(output))
}

/// History width for an explicit map: up to and including its highest source channel.
fn highest_mapped_channel_count(mapping: &[Option<usize>]) -> usize {
    mapping
        .iter()
        .flatten()
        .copied()
        .max()
        .map_or(0, |channel| channel + 1)
}

fn zeroed_history(stored_channels: usize, delay_frames: usize) -> Result<Vec<f32>, DryDelayError> {
    let samples = stored_channels
        .checked_mul(delay_frames)
        .ok_or(DryDelayError::InvalidLayout(
            "dry-delay history size overflow",
        ))?;
    let mut history = Vec::new();
    history.try_reserve_exact(samples)?;
    history.resize(samples, 0.0);
    Ok(history)
}

fn advance_cursor(cursor: usize, frames: usize, delay_frames: usize) -> usize {
    let frames_until_wrap = delay_frames - cursor;
    if frames >= frames_until_wrap {
        frames - frames_until_wrap
    } else {
        cursor + frames
    }
}
