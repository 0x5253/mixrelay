//! Fallible control-side storage preparation and disjoint render-time PCM borrows.
use super::{PortSlot, RenderPlanError, sample_count};

pub(super) fn reserve<T>(length: usize) -> Result<Vec<T>, RenderPlanError> {
    #[cfg(test)]
    if length != 0 && fault_injection::reservation_should_fail() {
        return Err(RenderPlanError::BufferPreparation(
            "injected reservation failure".into(),
        ));
    }
    mixrelay_audio_io::pcm_buffer::reserve(length)
        .map_err(|error| RenderPlanError::BufferPreparation(error.to_string()))
}

pub(super) fn filled(length: usize, value: f32) -> Result<Box<[f32]>, RenderPlanError> {
    let mut samples = reserve(length)?;
    samples.resize(length, value);
    Ok(samples.into_boxed_slice())
}

pub(super) fn port_pair(
    arena: &mut [f32],
    input: PortSlot,
    output: PortSlot,
    frames: usize,
) -> (&[f32], &mut [f32]) {
    let input_samples = sample_count(frames, input.channels);
    let output_samples = sample_count(frames, output.channels);
    // allocate_slots assigns disjoint spans, including a node's own input/output.
    // split_at_mut enforces that contract without raw pointers or aliasing promises.
    if input.offset < output.offset {
        let (before, after) = arena.split_at_mut(output.offset);
        (
            &before[input.offset..input.offset + input_samples],
            &mut after[..output_samples],
        )
    } else {
        let (before, after) = arena.split_at_mut(input.offset);
        (
            &after[..input_samples],
            &mut before[output.offset..output.offset + output_samples],
        )
    }
}

#[cfg(test)]
pub(super) mod fault_injection;
