//! Native device audio return path with bounded RAW frame budgets and source-local faults.
use mixrelay_audio_io::{
    frame_budget::FrameBudgetMapper,
    process::{ClockDomain, ProcessCycle},
    session::ClockAuthority,
};
use mixrelay_linux_pipewire::{
    DriverCycle, DriverError, DriverProcess, DriverProcessFailure, GraphInputs, NativeStream,
};

mod budget;
#[cfg(test)]
mod tests;

// Largest RAW transfer prepared off RT; retained input storage is a separate bound.
pub(super) const MAX_FRAMES: usize =
    mixrelay_linux_pipewire::StreamFormat::MAX_TRANSFER_FRAMES as usize;

pub(super) struct Loopback {
    pub(super) clock: std::sync::Arc<dyn crate::admission::ControlTime>,
    pub(super) render: Option<NativeStream>,
    pub(super) capture: Option<NativeStream>,
    pub(super) input: Box<[f32]>,
    pub(super) output: Box<[f32]>,
    pub(super) render_channels: usize,
    pub(super) capture_channels: usize,
    pub(super) mapper: FrameBudgetMapper,
    pub(super) sample_rate: u32,
    pub(super) failure: Option<DriverProcessFailure>,
}

#[cfg(test)]
impl Drop for Loopback {
    fn drop(&mut self) {
        // GraphDriver destroys the native filter and drains processing before this drop.
        for (role, stream) in [
            ("driver-render", &self.render),
            ("driver-capture", &self.capture),
        ] {
            if let Some(stream) = stream {
                let mut snapshot = mixrelay_linux_pipewire::TransferSnapshot::default();
                snapshot.include(stream);
                eprintln!("native_driver_pcm owner={role} snapshot={snapshot:?}");
            }
        }
    }
}

/// Straight channel projection used by the existing canonical device bus:
/// corresponding lanes pass unchanged, excess capture lanes are silent.
pub(super) fn project(input: &[f32], output: &mut [f32], render: usize, capture: usize) {
    output.fill(0.0);
    if render == 0 || capture == 0 {
        return;
    }
    for (source, destination) in input
        .chunks_exact(render)
        .zip(output.chunks_exact_mut(capture))
    {
        let width = render.min(capture);
        destination[..width].copy_from_slice(&source[..width]);
    }
}

/// Borrows matching-width PCM directly, otherwise projects into prepared capture storage.
pub(super) fn capture_samples<'a>(
    input: &'a [f32],
    output: &'a mut [f32],
    render: usize,
    capture: usize,
) -> &'a [f32] {
    if render == capture {
        return input;
    }
    project(input, output, render, capture);
    output
}

impl DriverProcess for Loopback {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        if let Some(render) = &mut self.render {
            render.bind_inputs(inputs).map_err(DriverError::Input)?;
        }
        if let Some(capture) = &self.capture {
            // Keep DSP unpublished until both native directions are negotiated.
            // This runs on control; PipeWire can still prepare the active graph.
            crate::admission::wait([None, None], self.clock.as_ref(), || {
                crate::admission::Evidence {
                    directions: [crate::admission::DirectionEvidence::Absent; 2],
                    ready: capture.ready(),
                    processing_failed: false,
                    disconnected: false,
                }
            })
            .map_err(|_| {
                DriverError::Input(
                    "loopback output format did not negotiate before DSP publication".into(),
                )
            })?;
        }
        Ok(())
    }

    fn process(&mut self, cycle: &DriverCycle) {
        if self.failure.is_some() {
            return;
        }
        if let Err(failure) = budget::validate(cycle, self.sample_rate) {
            self.failure = Some(failure);
            return;
        }
        let Ok(mapped) = self.mapper.map(ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::AudioServer,
                id: u64::from(cycle.clock_id),
                epoch: 0,
            },
            sample_rate: cycle.rate,
            frames: cycle.frames,
            first_frame: cycle.position,
            monotonic_ns: Some(cycle.monotonic_ns),
            discontinuity: false,
        }) else {
            self.failure = Some(DriverProcessFailure::InvalidPosition);
            return;
        };
        // Only complete native-clock frame budgets enter the same existing RAW FIFO.
        let Ok(frames) = usize::try_from(mapped.frames) else {
            self.failure = Some(budget::failure(cycle, self.sample_rate));
            return;
        };
        if frames == 0 {
            if let Some(capture) = &mut self.capture {
                capture.trigger_output();
            }
            return;
        }
        if frames > MAX_FRAMES {
            self.failure = Some(budget::failure(cycle, self.sample_rate));
            return;
        }
        let input = &mut self.input[..frames * self.render_channels];
        if let Some(render) = &mut self.render {
            render.read(input);
        }
        let output_count = if self.render_channels == self.capture_channels {
            0
        } else {
            frames * self.capture_channels
        };
        let output = &mut self.output[..output_count];
        let output = capture_samples(input, output, self.render_channels, self.capture_channels);
        if let Some(capture) = &mut self.capture {
            capture.write(output);
            capture.trigger_output();
        }
    }
    fn failure(&self) -> Option<DriverProcessFailure> {
        self.failure
    }
}
