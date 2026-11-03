//! Audio-owner cycle admission, external transfers and rendering of a prepared plan.
use super::{
    BlockProcessor, CycleAdmission, ExternalControlState, ExternalIo, Kernel, KernelResources,
    ProcessBlockError, RenderPhase, SampleFaults, apply_kernel, apply_routes, history,
    sample_count, sanitize_samples, sanitize_slots, silence_slots, validate_sink_lengths,
    validate_source_lengths,
};
use mixrelay_audio_io::process::{ProcessCycle, ProcessReport};

impl BlockProcessor {
    /// Processes one interleaved audio block without allocating or locking.
    ///
    /// Inputs and outputs must follow the order exposed by [`RenderPlan::source_bindings`]
    /// and [`RenderPlan::sink_bindings`].
    ///
    /// # Errors
    ///
    /// Returns [`ProcessBlockError`] before mutating output buffers when the frame count,
    /// binding count, or supplied buffer lengths do not match the prepared plan.
    pub fn process_block(
        &mut self,
        frames: u32,
        sources: &[&[f32]],
        sinks: &mut [&mut [f32]],
    ) -> Result<(), ProcessBlockError> {
        let frames = self.validate_buffers(frames, sources, sinks)?;
        self.sample_controls();
        for (binding, (source, destination)) in self
            .plan
            .source_bindings
            .iter()
            .zip(sources.iter().zip(&mut self.source_buffers))
        {
            let samples = sample_count(frames, binding.channels);
            destination[..samples].copy_from_slice(&source[..samples]);
        }
        self.process_loaded_block(frames);
        for (binding, (source, destination)) in self
            .plan
            .sink_bindings
            .iter()
            .zip(self.sink_buffers.iter().zip(sinks))
        {
            let samples = sample_count(frames, binding.channels);
            destination[..samples].copy_from_slice(&source[..samples]);
        }
        Ok(())
    }

    /// Reads external sources, processes one block, and writes external sinks without allocating.
    /// Returns `None` when native admission requests a retry: controls and adapter gates are
    /// observed, but no PCM, DSP, recording, meters, or cycle-tracker position are advanced.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessBlockError`] for an invalid clock/format/quantum or a
    /// native adapter reporting an impossible transfer size. Contract failures
    /// stop the owning processing session; they are not retryable audio blocks.
    pub fn process_external_cycle(
        &mut self,
        cycle: ProcessCycle,
        external: &mut impl ExternalIo,
    ) -> Result<Option<ProcessReport>, ProcessBlockError> {
        let Some(reset_history) = self.admit_external_cycle(cycle, external)? else {
            return Ok(None);
        };
        let frames = cycle.frames;
        let frames_usize = frames as usize;
        let mut report = self.read_external_sources(cycle, external)?;
        // An individual input invalidates only its downstream histories above.
        // A global clock loss invalidates the complete processing timeline.
        if reset_history {
            for binding in &self.plan.history {
                binding
                    .state
                    .reset(&mut self.effect_states, &mut self.dynamics_envelopes);
            }
            self.device_bus_state.fill(0.0);
            self.device_bus_cursor = 0;
            if let Some(bank) = &mut self.delay_bank {
                bank.reset();
            }
        }
        let external_source_faults = report.sample_faults.source_samples;
        report.sample_faults = self.process_loaded_block(frames_usize);
        report.sample_faults.source_samples = report
            .sample_faults
            .source_samples
            .saturating_add(external_source_faults);
        if external_source_faults != 0 {
            report.sample_faults.blocks = 1;
        }
        for (sink_index, ((binding, source), &control_index)) in self
            .plan
            .sink_bindings
            .iter()
            .zip(&self.sink_buffers)
            .zip(&self.plan.sink_controls)
            .enumerate()
        {
            if !self.controls[control_index].control.enabled {
                continue;
            }
            let samples = sample_count(frames_usize, binding.channels);
            let transfer = external.write_sink(binding, &source[..samples], &cycle);
            if transfer.frames > frames {
                return Err(ProcessBlockError::InvalidTransfer);
            }
            self.sink_latency_faults[sink_index] =
                transfer.discontinuity || transfer.frames < frames;
            report.output_overflows += u64::from(transfer.frames < frames);
            report.discontinuities += u64::from(transfer.discontinuity);
        }
        Ok(Some(report))
    }

    fn read_external_sources(
        &mut self,
        cycle: ProcessCycle,
        external: &mut impl ExternalIo,
    ) -> Result<ProcessReport, ProcessBlockError> {
        let frames = cycle.frames;
        let frames_usize = frames as usize;
        let mut report = ProcessReport::default();
        self.source_latency_faults.fill(false);
        self.sink_latency_faults.fill(false);
        for (source_index, ((binding, destination), &control_index)) in self
            .plan
            .source_bindings
            .iter()
            .zip(&mut self.source_buffers)
            .zip(&self.plan.source_controls)
            .enumerate()
        {
            let samples = sample_count(frames_usize, binding.channels);
            destination[..samples].fill(0.0);
            if !self.controls[control_index].control.enabled {
                continue;
            }
            let transfer = self.source_timelines[source_index].read(
                external,
                binding,
                &mut destination[..samples],
                &cycle,
            )?;
            // An adapter may fill only a prefix. Never process stale data from
            // another cycle, even if it wrote beyond the reported prefix.
            destination[sample_count(transfer.frames as usize, binding.channels)..samples]
                .fill(0.0);
            let starved = transfer.activity == mixrelay_audio_io::process::InputActivity::Active
                && transfer.frames < frames;
            report.input_starvations += u64::from(starved);
            self.source_latency_faults[source_index] = transfer.discontinuity || starved;
            report.discontinuities += u64::from(transfer.discontinuity);
            report.sample_faults.source_samples = report
                .sample_faults
                .source_samples
                .saturating_add(transfer.source_sample_faults);
            if transfer.discontinuity {
                let targets = &self.plan.source_history[source_index];
                for range in &targets.device_buses {
                    self.device_bus_state[range.clone()].fill(0.0);
                }
                if let Some(bank) = &mut self.delay_bank {
                    bank.reset_routes(&targets.delay_routes);
                }
                for &state in &targets.histories {
                    state.reset(&mut self.effect_states, &mut self.dynamics_envelopes);
                }
            }
        }
        Ok(report)
    }

    fn admit_external_cycle(
        &mut self,
        cycle: ProcessCycle,
        external: &mut impl ExternalIo,
    ) -> Result<Option<bool>, ProcessBlockError> {
        if cycle.frames > self.plan.format.block_frames {
            return Err(ProcessBlockError::FrameCountExceeded {
                maximum: self.plan.format.block_frames,
                found: cycle.frames,
            });
        }
        // Validation is speculative: a capacity retry must accept the same timeline coordinates
        // next time. The fixed-size candidate contains no heap storage or native resources.
        let mut candidate = self.cycle_tracker.clone();
        candidate
            .accept(
                cycle,
                mixrelay_audio_io::session::SessionFormat {
                    sample_rate: self.plan.format.sample_rate,
                    block_frames: self.plan.format.block_frames,
                },
            )
            .map_err(ProcessBlockError::InvalidCycle)?;
        self.sample_controls();
        match external.begin_cycle(
            &cycle,
            ExternalControlState::new(&self.plan, &self.controls),
        ) {
            CycleAdmission::Retry => Ok(None),
            CycleAdmission::Process => {
                self.cycle_tracker = candidate;
                Ok(Some(cycle.discontinuity))
            }
            CycleAdmission::ClockHandoff => {
                self.cycle_tracker = candidate;
                Ok(Some(false))
            }
        }
    }

    fn process_loaded_block(&mut self, frames: usize) -> SampleFaults {
        let mut faults = SampleFaults::default();
        for ((binding, source), &control_index) in self
            .plan
            .source_bindings
            .iter()
            .zip(&mut self.source_buffers)
            .zip(&self.plan.source_controls)
        {
            if self.controls[control_index].control.enabled {
                faults.source_samples +=
                    sanitize_samples(&mut source[..sample_count(frames, binding.channels)]);
            }
        }
        if let Some(bank) = &mut self.delay_bank {
            bank.validate();
        }
        if frames == self.plan.format.block_frames as usize {
            self.arena.fill(0.0);
        } else {
            silence_slots(&mut self.arena, &self.plan.arena_slots, frames);
        }
        self.dynamics_minimum_gains.fill(1.0);
        for (sink, &control_index) in self.sink_buffers.iter_mut().zip(&self.plan.sink_controls) {
            if !self.controls[control_index].control.enabled {
                sink.fill(0.0);
            }
        }
        for phase in [
            RenderPhase::Sources,
            RenderPhase::Processing,
            RenderPhase::Sinks,
        ] {
            if matches!(phase, RenderPhase::Sinks) {
                apply_routes(
                    &mut self.arena,
                    &self.plan.sink_routes,
                    &self.controls,
                    frames,
                    self.delay_bank.as_deref_mut(),
                );
            }
            self.process_phase(phase, frames, &mut faults);
        }
        if !self.device_bus_state.is_empty() {
            // All buses read before publishing at the same prepared-quantum
            // delay, independent of how the native callback partitions frames.
            self.device_bus_cursor =
                (self.device_bus_cursor + frames) % self.plan.format.block_frames as usize;
        }
        self.capture_and_meter(frames);
        faults.blocks = u64::from(
            faults.source_samples != 0 || faults.input_samples != 0 || faults.output_samples != 0,
        );
        faults
    }

    fn sample_controls(&mut self) {
        if !self.plan.controls.sample(
            &mut self.controls,
            &mut self.control_scratch,
            &mut self.sampled_control_epoch,
        ) {
            return;
        }
        // Device snapshots are persistent audio, unlike the per-block cleared arena. OFF must
        // forget its last quantum so re-enabling cannot replay audio from before the pause.
        if let Some(bank) = &mut self.delay_bank {
            bank.observe_controls(&self.controls);
        }
        for binding in &self.plan.device_bus_bindings {
            if !self.controls[binding.control_index].control.enabled {
                self.device_bus_state[binding.samples.clone()].fill(0.0);
            }
        }
    }

    fn process_phase(&mut self, phase: RenderPhase, frames: usize, faults: &mut SampleFaults) {
        for &index in self.plan.schedule.nodes(phase) {
            let step = &self.plan.steps[index];
            let control = self.controls[index];
            if !control.control.enabled {
                for kernel in step.kernels(phase) {
                    if let Kernel::Hosted { processor, .. } = kernel {
                        self.hosted.suspend(*processor);
                    }
                    let history = match kernel {
                        Kernel::Effect { state, .. } => Some(history::State::Effect(*state)),
                        Kernel::Compressor {
                            state_index,
                            bypass,
                            ..
                        }
                        | Kernel::Limiter {
                            state_index,
                            bypass,
                            ..
                        } => {
                            // OFF settles the parameter transition; timeline resets keep it.
                            self.dynamics_wet[*state_index] = f32::from(!*bypass);
                            Some(history::State::Dynamics(*state_index))
                        }
                        _ => None,
                    };
                    if let Some(state) = history {
                        state.reset(&mut self.effect_states, &mut self.dynamics_envelopes);
                    }
                }
                continue;
            }
            if matches!(phase, RenderPhase::Processing) {
                apply_routes(
                    &mut self.arena,
                    &step.incoming_routes,
                    &self.controls,
                    frames,
                    self.delay_bank.as_deref_mut(),
                );
            }
            // Routes can overflow even when each upstream sample is finite.
            // Protect every consumer, including sidechains and native/recorder sinks.
            faults.input_samples += sanitize_slots(&mut self.arena, &step.inputs, frames);
            if matches!(phase, RenderPhase::Sinks) && control.control.muted {
                silence_slots(&mut self.arena, &step.inputs, frames);
            }
            for kernel in step.kernels(phase) {
                apply_kernel(
                    KernelResources {
                        arena: &mut self.arena,
                        sources: &self.source_buffers,
                        sinks: &mut self.sink_buffers,
                        dynamics_envelopes: &mut self.dynamics_envelopes,
                        dynamics_minimum_gains: &mut self.dynamics_minimum_gains,
                        dynamics_wet: &mut self.dynamics_wet,
                        effect_states: &mut self.effect_states,
                        effects: &self.plan.effects,
                        device_bus_state: &mut self.device_bus_state,
                        device_bus_cursor: self.device_bus_cursor,
                        hosted: &mut self.hosted,
                    },
                    kernel,
                    control.linear_gain,
                    frames,
                );
            }
            // Source gain, mixing and hosted processors share this boundary.
            // Invalid output must not become the next node's recursive history.
            faults.output_samples += sanitize_slots(&mut self.arena, &step.outputs, frames);
            if !matches!(phase, RenderPhase::Sinks) && control.control.muted {
                silence_slots(&mut self.arena, &step.outputs, frames);
            }
        }
    }

    fn capture_and_meter(&self, frames: usize) {
        for ((binding, recorder), &control_index) in self
            .plan
            .recorder_bindings
            .iter()
            .zip(&self.recorders)
            .zip(&self.plan.recorder_controls)
        {
            if !self.controls[control_index].control.enabled {
                continue;
            }
            let samples = sample_count(frames, binding.channels);
            recorder.capture(
                &self.arena[binding.slot.offset..binding.slot.offset + samples],
                self.plan.format.sample_rate,
            );
        }
        if let Some(meters) = &self.meters {
            if frames == 0 {
                return;
            }
            let revision = self
                .plan
                .revision
                .max(self.plan.controls.applied_revision());
            meters.begin_publication();
            for (index, binding) in self.plan.meter_bindings.iter().enumerate() {
                let samples = sample_count(frames, binding.channels);
                meters.record_interleaved(
                    revision,
                    index,
                    binding.channels,
                    &self.arena[binding.slot.offset..binding.slot.offset + samples],
                );
                if let Some(dynamics) = binding.dynamics {
                    meters.record_dynamics(
                        revision,
                        index,
                        &self.arena[dynamics.input.offset
                            ..dynamics.input.offset
                                + sample_count(frames, dynamics.input.channels)],
                        crate::DynamicsMeterBlock {
                            channels: dynamics.input.channels,
                            minimum_gain: self.dynamics_minimum_gains[dynamics.state_index],
                            threshold_db: dynamics.threshold_db,
                        },
                    );
                }
            }
            meters.finish_publication(revision);
        }
    }

    fn validate_buffers(
        &self,
        frames: u32,
        sources: &[&[f32]],
        sinks: &[&mut [f32]],
    ) -> Result<usize, ProcessBlockError> {
        if frames > self.plan.format.block_frames {
            return Err(ProcessBlockError::FrameCountExceeded {
                maximum: self.plan.format.block_frames,
                found: frames,
            });
        }
        if sources.len() != self.plan.source_bindings.len() {
            return Err(ProcessBlockError::SourceCountMismatch {
                expected: self.plan.source_bindings.len(),
                found: sources.len(),
            });
        }
        if sinks.len() != self.plan.sink_bindings.len() {
            return Err(ProcessBlockError::SinkCountMismatch {
                expected: self.plan.sink_bindings.len(),
                found: sinks.len(),
            });
        }

        let frames = frames as usize;
        validate_source_lengths(frames, &self.plan.source_bindings, sources)?;
        validate_sink_lengths(frames, &self.plan.sink_bindings, sinks)?;
        Ok(frames)
    }
}
