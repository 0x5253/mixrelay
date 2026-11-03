//! Fallible control-side storage preparation and disjoint render-time PCM borrows.
use super::{PortSlot, RenderPlanError, sample_count};

pub(super) fn reserve<T>(length: usize) -> Result<Vec<T>, RenderPlanError> {
    #[cfg(test)]
    if length != 0 && reservation_should_fail() {
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

#[cfg(test)]
std::thread_local! {
    static RESERVATIONS_BEFORE_FAILURE: std::cell::Cell<Option<usize>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(test)]
fn reservation_should_fail() -> bool {
    RESERVATIONS_BEFORE_FAILURE.with(|remaining| match remaining.get() {
        Some(0) => {
            remaining.set(None);
            true
        }
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
        None => false,
    })
}

#[cfg(test)]
pub(super) fn fail_after<T>(count: usize, work: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RESERVATIONS_BEFORE_FAILURE.with(|remaining| remaining.set(None));
        }
    }
    RESERVATIONS_BEFORE_FAILURE.with(|remaining| {
        assert!(remaining.replace(Some(count)).is_none());
    });
    let _reset = Reset;
    work()
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
