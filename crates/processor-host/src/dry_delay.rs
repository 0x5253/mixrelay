//! Preallocated fixed-frame dry history for a hosted worker's late-response fallback.
//!
//! The delay models the known IPC block latency only. It does not include plug-in intrinsic
//! latency and is not a graph-level parallel-path compensation mechanism.

use mixrelay_audio_graph::{
    MAX_PROVIDER_BUSES_PER_DIRECTION, ProviderBusKind, ProviderPorts, ProviderPortsError,
};

const MAX_DRY_DELAY_BYTES: usize = 16 * 1024 * 1024;

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
    /// Build a flat-prefix dry-delay fixture for unit tests.
    ///
    /// Input and output slices passed to [`Self::advance`] are frame-interleaved: each frame
    /// contains all channels in channel order. The ring stores the shared leading channels;
    /// extra outputs are filled with silence. Production callers must use [`Self::for_ports`]
    /// so dry routing follows the declared main-bus topology.
    ///
    /// # Errors
    /// Rejects invalid or oversized dimensions and reports a failed history allocation.
    #[cfg(test)]
    pub(crate) fn new(
        input_channels: usize,
        output_channels: usize,
        delay_frames: usize,
    ) -> Result<Self, DryDelayError> {
        Self::with_mapping(input_channels, output_channels, delay_frames, None)
    }

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
        let mapping = dry_channel_mapping(ports)?;
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

    fn with_mapping(
        input_channels: usize,
        output_channels: usize,
        delay_frames: usize,
        mut mapping: Option<Vec<Option<usize>>>,
    ) -> Result<Self, DryDelayError> {
        let (canonical_channels, _) = checked_sizes(input_channels, output_channels, delay_frames)
            .map_err(DryDelayError::InvalidLayout)?;
        if let Some(channels) = &mapping {
            if channels.len() != output_channels
                || channels
                    .iter()
                    .flatten()
                    .any(|channel| *channel >= input_channels)
            {
                return Err(DryDelayError::InvalidLayout(
                    "dry-delay channel map does not match its channel dimensions",
                ));
            }
            // Normalize an equivalent flat-prefix map to preserve the direct-copy fast path.
            if channels
                .iter()
                .enumerate()
                .all(|(output, input)| *input == (output < canonical_channels).then_some(output))
            {
                mapping = None;
            }
        }
        let stored_channels = mapping.as_ref().map_or(canonical_channels, |channels| {
            channels
                .iter()
                .flatten()
                .copied()
                .max()
                .map_or(0, |channel| channel + 1)
        });
        let history_samples =
            stored_channels
                .checked_mul(delay_frames)
                .ok_or(DryDelayError::InvalidLayout(
                    "dry-delay history size overflow",
                ))?;
        let mut history = Vec::new();
        history.try_reserve_exact(history_samples)?;
        history.resize(history_samples, 0.0);
        Ok(Self {
            input_channels,
            output_channels,
            stored_channels,
            delay_frames,
            cursor: 0,
            valid_history_frames: 0,
            mapping,
            history,
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
        mut output: Option<&mut [f32]>,
        frames: usize,
    ) -> bool {
        if frames == 0 || frames > self.delay_frames {
            return false;
        }
        let Some(input_samples) = self.input_channels.checked_mul(frames) else {
            return false;
        };
        let Some(output_samples) = self.output_channels.checked_mul(frames) else {
            return false;
        };
        if input.len() < input_samples
            || output
                .as_ref()
                .is_some_and(|slice| slice.len() < output_samples)
        {
            return false;
        }

        if self.mapping.is_none()
            && self.input_channels == self.output_channels
            && self.input_channels == self.stored_channels
            && self.output_channels == self.stored_channels
            && let Some(slice) = output.as_deref_mut()
        {
            self.advance_equal_layout(input, slice, frames);
            return true;
        }
        if self.mapping.is_none()
            && self.input_channels == self.stored_channels
            && self.output_channels == self.stored_channels
            && output.is_none()
        {
            self.copy_input_to_history(self.cursor, &input[..input_samples]);
            self.advance_position(frames);
            return true;
        }
        self.advance_mismatched_layout(input, output, frames);
        true
    }

    fn advance_equal_layout(&mut self, input: &[f32], output: &mut [f32], frames: usize) {
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
        self.copy_input_to_history(self.cursor, &input[..total_samples]);
        self.advance_position(frames);
    }

    fn advance_mismatched_layout(
        &mut self,
        input: &[f32],
        mut output: Option<&mut [f32]>,
        frames: usize,
    ) {
        let mut cursor = self.cursor;
        let mut valid_history_frames = self.valid_history_frames;
        for frame in 0..frames {
            let history_is_valid = valid_history_frames == self.delay_frames;
            let input_start = frame * self.input_channels;
            let output_start = frame * self.output_channels;
            let history_start = cursor * self.stored_channels;
            let history_end = history_start + self.stored_channels;
            if let Some(slice) = output.as_deref_mut() {
                for output_channel in 0..self.output_channels {
                    let output_index = output_start + output_channel;
                    let Some(input_channel) = self.source_channel(output_channel) else {
                        slice[output_index] = 0.0;
                        continue;
                    };
                    slice[output_index] = if history_is_valid {
                        self.history[history_start + input_channel]
                    } else {
                        0.0
                    };
                }
            }
            if self.stored_channels > 0 {
                self.history[history_start..history_end]
                    .copy_from_slice(&input[input_start..input_start + self.stored_channels]);
            }
            cursor += 1;
            if cursor == self.delay_frames {
                cursor = 0;
            }
            if valid_history_frames < self.delay_frames {
                valid_history_frames += 1;
            }
        }
        self.cursor = cursor;
        self.valid_history_frames = valid_history_frames;
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

/// Maps each flattened output channel to its corresponding flattened main-input channel.
/// Sidechain/auxiliary outputs and unmatched main outputs map to `None` (silence).
///
/// # Errors
/// Returns a provider-port validation error for malformed or oversized topology.
pub(crate) fn dry_channel_mapping(
    ports: &ProviderPorts,
) -> Result<Vec<Option<usize>>, DryDelayError> {
    ports.validate()?;
    let output_channels = usize::from(ports.total_output_channels()?);
    let mut mapping = Vec::new();
    mapping.try_reserve_exact(output_channels)?;
    mapping.resize(output_channels, None);

    let mut main_inputs = [(0_usize, 0_usize); MAX_PROVIDER_BUSES_PER_DIRECTION];
    let mut main_input_count = 0;
    let mut input_offset = 0_usize;
    for bus in &ports.inputs {
        let channels = usize::from(bus.channels);
        if bus.kind == ProviderBusKind::Main {
            main_inputs[main_input_count] = (input_offset, channels);
            main_input_count += 1;
        }
        input_offset += channels;
    }

    let mut main_output_index = 0;
    let mut output_offset = 0_usize;
    for bus in &ports.outputs {
        let output_width = usize::from(bus.channels);
        if bus.kind == ProviderBusKind::Main {
            if let Some((input_start, input_width)) = main_inputs.get(main_output_index).copied() {
                for channel in 0..input_width.min(output_width) {
                    mapping[output_offset + channel] = Some(input_start + channel);
                }
            }
            main_output_index += 1;
        }
        output_offset += output_width;
    }
    Ok(mapping)
}

fn advance_cursor(cursor: usize, frames: usize, delay_frames: usize) -> usize {
    let frames_until_wrap = delay_frames - cursor;
    if frames >= frames_until_wrap {
        frames - frames_until_wrap
    } else {
        cursor + frames
    }
}

fn checked_sizes(
    input_channels: usize,
    output_channels: usize,
    delay_frames: usize,
) -> Result<(usize, usize), &'static str> {
    if output_channels == 0 {
        return Err("dry-delay output channel count must be nonzero");
    }
    if delay_frames == 0 {
        return Err("dry-delay frame count must be nonzero");
    }
    let input_bytes = checked_sample_bytes(input_channels, delay_frames)
        .ok_or("dry-delay input stride size overflow")?;
    if input_bytes > MAX_DRY_DELAY_BYTES {
        return Err("dry-delay input stride exceeds the 16 MiB limit");
    }
    let output_bytes = checked_sample_bytes(output_channels, delay_frames)
        .ok_or("dry-delay output stride size overflow")?;
    if output_bytes > MAX_DRY_DELAY_BYTES {
        return Err("dry-delay output stride exceeds the 16 MiB limit");
    }
    let stored_channels = input_channels.min(output_channels);
    let history_samples = stored_channels
        .checked_mul(delay_frames)
        .ok_or("dry-delay history size overflow")?;
    // `stored_channels` is no greater than either side, so its history bytes are bounded by the
    // input/output stride caps above.
    Ok((stored_channels, history_samples))
}

fn checked_sample_bytes(channels: usize, frames: usize) -> Option<usize> {
    channels
        .checked_mul(frames)?
        .checked_mul(std::mem::size_of::<f32>())
}

#[cfg(test)]
mod tests {
    use super::{DryDelay, MAX_DRY_DELAY_BYTES, checked_sizes, dry_channel_mapping};
    use mixrelay_audio_graph::{
        ProviderBus, ProviderBusKind, ProviderChannelFormat, ProviderPorts,
    };

    fn bus(id: &str, kind: ProviderBusKind, channels: u16) -> ProviderBus {
        ProviderBus {
            id: id.into(),
            name: id.to_owned(),
            kind,
            channels,
            channel_format: ProviderChannelFormat::Discrete,
        }
    }

    fn ports(inputs: Vec<ProviderBus>, outputs: Vec<ProviderBus>) -> ProviderPorts {
        ProviderPorts {
            layout_id: "fixture-layout".into(),
            inputs,
            outputs,
        }
    }

    fn assert_samples(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(actual.to_bits(), expected.to_bits(), "sample {index}");
        }
    }

    fn assert_silent(samples: &[f32]) {
        for (index, sample) in samples.iter().enumerate() {
            assert_eq!(sample.to_bits(), 0.0_f32.to_bits(), "sample {index}");
        }
    }

    fn test_frame_value(frames: usize) -> f32 {
        f32::from(u16::try_from(frames).expect("test frame count fits u16"))
    }

    #[test]
    fn starts_silent_and_returns_samples_after_exact_delay() {
        let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
        let mut first = [9.0; 2];
        assert!(delay.advance(&[1.0, 2.0], Some(&mut first), 2));
        assert_silent(&first);

        let mut second = [9.0; 3];
        assert!(delay.advance(&[3.0, 4.0, 5.0], Some(&mut second), 3));
        assert_samples(&second, &[0.0, 1.0, 2.0]);
    }

    #[test]
    fn split_calls_match_full_calls_for_interleaved_samples() {
        let source = [
            0.0, 10.0, 1.0, 11.0, 2.0, 12.0, 3.0, 13.0, 4.0, 14.0, 5.0, 15.0, 6.0, 16.0, 7.0, 17.0,
        ];
        let mut full_delay = DryDelay::new(2, 2, 4).expect("valid delay");
        let mut split_delay = DryDelay::new(2, 2, 4).expect("valid delay");
        let mut full_output = Vec::new();
        for start in [0, 4] {
            let mut block = [0.0; 8];
            assert!(full_delay.advance(&source[start * 2..(start + 4) * 2], Some(&mut block), 4));
            full_output.extend_from_slice(&block);
        }

        let mut split_output = Vec::new();
        let mut start = 0;
        for frames in [1, 3, 2, 2] {
            let mut block = vec![0.0; frames * 2];
            assert!(split_delay.advance(
                &source[start * 2..(start + frames) * 2],
                Some(&mut block),
                frames
            ));
            split_output.extend_from_slice(&block);
            start += frames;
        }
        assert_samples(&split_output, &full_output);
    }

    #[test]
    fn longer_input_and_output_slices_keep_their_unused_suffixes() {
        let mut delay = DryDelay::new(2, 2, 2).expect("valid delay");
        let mut first_output = [7.0; 6];
        assert!(delay.advance(
            &[1.0, 10.0, 2.0, 20.0, 90.0, 91.0],
            Some(&mut first_output),
            2
        ));
        assert_silent(&first_output[..4]);
        assert_samples(&first_output[4..], &[7.0; 2]);

        let mut second_output = [8.0; 6];
        assert!(delay.advance(
            &[3.0, 30.0, 4.0, 40.0, 80.0, 81.0],
            Some(&mut second_output),
            2
        ));
        assert_samples(&second_output[..4], &[1.0, 10.0, 2.0, 20.0]);
        assert_samples(&second_output[4..], &[8.0; 2]);
    }

    #[test]
    fn wrap_preserves_delay_across_many_variable_slices() {
        let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
        let mut frame = 0;
        for frames in [1, 2, 3, 1, 2, 3, 2, 1, 3] {
            let input: Vec<f32> = (0..frames)
                .map(|offset| test_frame_value(frame + offset))
                .collect();
            let mut output = vec![-1.0; frames];
            assert!(delay.advance(&input, Some(&mut output), frames));
            for (offset, actual) in output.iter().enumerate() {
                let current = frame + offset;
                let expected = if current < 3 {
                    0.0
                } else {
                    test_frame_value(current - 3)
                };
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
            frame += frames;
        }
    }

    #[test]
    fn channel_mismatch_keeps_shared_channels_and_silences_extra_outputs() {
        let mut more_outputs = DryDelay::new(2, 3, 2).expect("valid delay");
        let mut ignored = [9.0; 6];
        assert!(more_outputs.advance(&[1.0, 10.0, 2.0, 20.0], Some(&mut ignored), 2));
        assert_silent(&ignored);
        let mut output = [9.0; 6];
        assert!(more_outputs.advance(&[3.0, 30.0, 4.0, 40.0], Some(&mut output), 2));
        assert_samples(&output, &[1.0, 10.0, 0.0, 2.0, 20.0, 0.0]);

        let mut fewer_outputs = DryDelay::new(3, 2, 2).expect("valid delay");
        let mut first = [0.0; 4];
        assert!(fewer_outputs.advance(&[5.0, 50.0, 500.0, 6.0, 60.0, 600.0], Some(&mut first), 2));
        let mut second = [0.0; 4];
        assert!(fewer_outputs.advance(&[7.0, 70.0, 700.0, 8.0, 80.0, 800.0], Some(&mut second), 2));
        assert_samples(&second, &[5.0, 50.0, 6.0, 60.0]);
    }

    #[test]
    fn provider_dry_map_pairs_main_bus_ordinals_and_excludes_auxiliary_buses() {
        let ports = ports(
            vec![
                bus("side-in", ProviderBusKind::Sidechain, 1),
                bus("main-in-0", ProviderBusKind::Main, 2),
                bus("aux-in", ProviderBusKind::Auxiliary, 2),
                bus("main-in-1", ProviderBusKind::Main, 1),
            ],
            vec![
                bus("aux-out", ProviderBusKind::Auxiliary, 2),
                bus("main-out-0", ProviderBusKind::Main, 2),
                bus("side-out", ProviderBusKind::Sidechain, 1),
                bus("main-out-1", ProviderBusKind::Main, 1),
                bus("unmatched-main", ProviderBusKind::Main, 1),
            ],
        );

        assert_eq!(
            dry_channel_mapping(&ports).unwrap(),
            vec![None, None, Some(1), Some(2), None, Some(5), None]
        );
    }

    #[test]
    fn provider_dry_delay_forwards_only_main_inputs_and_keeps_other_outputs_silent() {
        let ports = ports(
            vec![
                bus("side-in", ProviderBusKind::Sidechain, 1),
                bus("main-in", ProviderBusKind::Main, 2),
                bus("aux-in", ProviderBusKind::Auxiliary, 2),
            ],
            vec![
                bus("aux-out", ProviderBusKind::Auxiliary, 2),
                bus("main-out", ProviderBusKind::Main, 2),
                bus("side-out", ProviderBusKind::Sidechain, 1),
                bus("unmatched-main", ProviderBusKind::Main, 1),
            ],
        );
        let mut delay = DryDelay::for_ports(&ports, 2).expect("valid provider layout");
        assert_eq!(
            delay.mapping,
            Some([None, None, Some(1), Some(2), None, None].to_vec())
        );

        let mut first = [7.0; 6];
        assert!(delay.advance(&[900.0, 10.0, 20.0, 800.0, 801.0], Some(&mut first), 1));
        assert_samples(&first, &[0.0; 6]);
        let mut second = [7.0; 6];
        assert!(delay.advance(&[901.0, 11.0, 21.0, 810.0, 811.0], Some(&mut second), 1));
        assert_samples(&second, &[0.0; 6]);
        let mut third = [7.0; 6];
        assert!(delay.advance(&[902.0, 12.0, 22.0, 820.0, 821.0], Some(&mut third), 1));
        assert_samples(&third, &[0.0, 0.0, 10.0, 20.0, 0.0, 0.0]);
    }

    #[test]
    fn provider_dry_delay_none_reset_and_variable_slices_preserve_only_main_mapping() {
        let ports = ports(
            vec![
                bus("side-in", ProviderBusKind::Sidechain, 1),
                bus("main-in", ProviderBusKind::Main, 2),
            ],
            vec![
                bus("main-out", ProviderBusKind::Main, 2),
                bus("aux-out", ProviderBusKind::Auxiliary, 1),
            ],
        );
        let mut delay = DryDelay::for_ports(&ports, 3).expect("valid provider layout");
        assert!(delay.advance(&[99.0, 1.0, 2.0], None, 1));
        let mut first = [7.0; 6];
        assert!(delay.advance(&[98.0, 3.0, 4.0, 97.0, 5.0, 6.0], Some(&mut first), 2));
        assert_samples(&first, &[0.0; 6]);
        let mut second = [7.0; 3];
        assert!(delay.advance(&[96.0, 7.0, 8.0], Some(&mut second), 1));
        assert_samples(&second, &[1.0, 2.0, 0.0]);

        delay.reset();
        let mut reset = [7.0; 3];
        assert!(delay.advance(&[95.0, 9.0, 10.0], Some(&mut reset), 1));
        assert_samples(&reset, &[0.0; 3]);
        assert!(delay.advance(&[94.0, 11.0, 12.0], None, 1));
        let mut after_reset = [7.0; 3];
        assert!(delay.advance(&[93.0, 13.0, 14.0], Some(&mut after_reset), 1));
        assert_samples(&after_reset, &[0.0; 3]);
        let mut refilled = [7.0; 3];
        assert!(delay.advance(&[92.0, 15.0, 16.0], Some(&mut refilled), 1));
        assert_samples(&refilled, &[9.0, 10.0, 0.0]);
    }

    #[test]
    fn provider_dry_delay_preserves_direct_copy_fast_path_for_single_main_bus() {
        let ports = ProviderPorts::single_bus("stereo", 2, 2);
        let mut delay = DryDelay::for_ports(&ports, 2).expect("valid single-main layout");
        assert!(delay.mapping.is_none());
        let input = [1.0, 10.0, 2.0, 20.0];
        let mut output = [7.0; 4];
        assert!(delay.advance(&input, Some(&mut output), 2));
        assert_samples(&output, &[0.0; 4]);
        assert!(delay.advance(&input, Some(&mut output), 2));
        assert_samples(&output, &input);
    }

    #[test]
    fn mapped_advance_reset_and_reprime_allocate_nothing() {
        let ports = ports(
            vec![
                bus("side-in", ProviderBusKind::Sidechain, 1),
                bus("main-in", ProviderBusKind::Main, 2),
            ],
            vec![
                bus("main-out", ProviderBusKind::Main, 2),
                bus("aux-out", ProviderBusKind::Auxiliary, 1),
            ],
        );
        let mut delay = DryDelay::for_ports(&ports, 4).expect("valid provider layout");
        let input = [
            99.0, 1.0, 2.0, 98.0, 3.0, 4.0, 97.0, 5.0, 6.0, 96.0, 7.0, 8.0,
        ];
        let mut output = [7.0; 12];
        let mut accepted = false;
        let allocations = allocation_counter::measure(|| {
            accepted = delay.advance(&input, Some(&mut output), 4);
            delay.reset();
            accepted &= delay.advance(&input, None, 4);
        });
        assert!(accepted);
        assert_eq!(allocations.count_total, 0);
        assert_eq!(allocations.bytes_total, 0);
    }

    #[test]
    fn invalid_provider_topology_is_rejected_before_map_construction() {
        let invalid = ports(
            vec![
                bus("duplicate", ProviderBusKind::Main, 1),
                bus("duplicate", ProviderBusKind::Sidechain, 1),
            ],
            vec![bus("main-out", ProviderBusKind::Main, 1)],
        );
        assert!(dry_channel_mapping(&invalid).is_err());
        assert!(DryDelay::for_ports(&invalid, 2).is_err());
    }

    #[test]
    fn zero_input_generator_fallback_is_silent() {
        let mut delay = DryDelay::new(0, 2, 3).expect("zero-input fallback is valid");
        let mut output = [7.0; 4];
        assert!(delay.advance(&[], Some(&mut output), 2));
        assert_silent(&output);
    }

    #[test]
    fn no_output_call_advances_history() {
        let mut delay = DryDelay::new(1, 1, 2).expect("valid delay");
        assert!(delay.advance(&[5.0, 6.0], None, 2));
        let mut output = [0.0; 2];
        assert!(delay.advance(&[7.0, 8.0], Some(&mut output), 2));
        assert_samples(&output, &[5.0, 6.0]);
    }

    #[test]
    fn reset_after_partial_history_refills_silently_across_variable_slices() {
        let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
        let mut old_output = [9.0; 2];
        assert!(delay.advance(&[90.0, 91.0], Some(&mut old_output), 2));
        assert_silent(&old_output);

        delay.reset();
        let mut first = [9.0; 1];
        assert!(delay.advance(&[10.0], Some(&mut first), 1));
        assert_silent(&first);
        let mut next = [9.0; 2];
        assert!(delay.advance(&[20.0, 30.0], Some(&mut next), 2));
        assert_silent(&next);
        let mut refilled = [9.0; 1];
        assert!(delay.advance(&[40.0], Some(&mut refilled), 1));
        assert_samples(&refilled, &[10.0]);
    }

    #[test]
    fn reset_after_full_history_hides_stale_samples_and_none_primes_ring() {
        let mut delay = DryDelay::new(1, 1, 3).expect("valid delay");
        let mut startup = [9.0; 3];
        assert!(delay.advance(&[1.0, 2.0, 3.0], Some(&mut startup), 3));
        assert_silent(&startup);
        let mut steady = [9.0; 1];
        assert!(delay.advance(&[4.0], Some(&mut steady), 1));
        assert_samples(&steady, &[1.0]);

        delay.reset();
        assert!(delay.advance(&[10.0], None, 1));
        let mut second = [9.0; 1];
        assert!(delay.advance(&[20.0], Some(&mut second), 1));
        assert_silent(&second);
        let mut third = [9.0; 1];
        assert!(delay.advance(&[30.0], Some(&mut third), 1));
        assert_silent(&third);
        let mut refilled = [9.0; 1];
        assert!(delay.advance(&[40.0], Some(&mut refilled), 1));
        assert_samples(&refilled, &[10.0]);
    }

    #[test]
    fn zero_input_reset_remains_silent() {
        let mut delay = DryDelay::new(0, 2, 4).expect("zero-input fallback is valid");
        let mut output = [3.0; 4];
        assert!(delay.advance(&[], None, 2));
        delay.reset();
        assert!(delay.advance(&[], Some(&mut output), 2));
        assert_silent(&output);
    }

    #[test]
    fn reset_and_reprime_allocate_nothing_for_supported_channel_counts() {
        for channels in [1, 2, 32, 64] {
            let frames = 8;
            let mut delay = DryDelay::new(channels, channels, frames).expect("valid delay");
            let input = vec![0.5; channels * frames];
            let mut output = vec![7.0; channels * frames];
            assert!(delay.advance(&input, Some(&mut output), frames));
            let mut accepted = false;
            let allocations = allocation_counter::measure(|| {
                delay.reset();
                accepted = delay.advance(&input, Some(&mut output), frames);
            });
            assert!(accepted);
            assert_silent(&output);
            assert_eq!(allocations.count_total, 0);
            assert_eq!(allocations.bytes_total, 0);
        }
    }

    #[test]
    fn invalid_requests_leave_history_and_output_unchanged() {
        let mut delay = DryDelay::new(1, 1, 2).expect("valid delay");
        let mut startup = [0.0; 1];
        assert!(delay.advance(&[1.0], Some(&mut startup), 1));
        let mut output = [77.0; 2];
        assert!(!delay.advance(&[], Some(&mut output), 0));
        assert_samples(&output, &[77.0; 2]);
        assert!(!delay.advance(&[2.0], Some(&mut output), 2));
        assert_samples(&output, &[77.0; 2]);
        assert!(!delay.advance(&[2.0, 3.0, 4.0], Some(&mut output[..1]), 2));
        assert_samples(&output, &[77.0; 2]);
        assert!(!delay.advance(&[2.0, 3.0, 4.0], Some(&mut output), 3));
        assert_samples(&output, &[77.0; 2]);

        let mut valid = [0.0; 1];
        assert!(delay.advance(&[2.0], Some(&mut valid), 1));
        assert_silent(&valid);
        assert!(delay.advance(&[3.0], Some(&mut valid), 1));
        assert_samples(&valid, &[1.0]);
    }

    #[test]
    fn constructor_rejects_invalid_dimensions_without_allocating_history() {
        assert!(DryDelay::new(1, 0, 1).is_err());
        assert!(DryDelay::new(1, 1, 0).is_err());
        assert!(checked_sizes(usize::MAX, 1, 2).is_err());
        assert!(checked_sizes(0, usize::MAX, 2).is_err());
    }

    #[test]
    fn constructor_stride_and_history_limits_use_arithmetic_only() {
        let maximum_frames = MAX_DRY_DELAY_BYTES / std::mem::size_of::<f32>();
        assert_eq!(checked_sizes(1, 1, maximum_frames), Ok((1, maximum_frames)));
        assert!(checked_sizes(1, 1, maximum_frames + 1).is_err());
        assert_eq!(checked_sizes(0, 1, maximum_frames), Ok((0, 0)));
        assert!(checked_sizes(0, 1, maximum_frames + 1).is_err());
        assert!(checked_sizes(2, 1, maximum_frames / 2 + 1).is_err());
    }

    #[test]
    fn callback_allocates_nothing_for_supported_channel_counts() {
        for channels in [1, 2, 32, 64] {
            let frames = 8;
            let mut delay = DryDelay::new(channels, channels, frames).expect("valid delay");
            let input = vec![0.25; channels * frames];
            let mut output = vec![0.0; channels * frames];
            let mut accepted = true;
            let allocations = allocation_counter::measure(|| {
                for _ in 0..32 {
                    accepted &= delay.advance(&input, Some(&mut output), frames);
                    accepted &= delay.advance(&input, None, frames);
                }
            });
            assert!(accepted);
            assert_eq!(allocations.count_total, 0);
            assert_eq!(allocations.bytes_total, 0);
        }
    }
}
