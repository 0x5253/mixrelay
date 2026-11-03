//! Preparation and ownership transfer of a block processor's retained resources.
use std::{fmt, sync::Arc};

use mixrelay_audio_graph::{ExecutionPlan, NodeConfig};

use super::{
    BlockProcessor, CompensationState, DelayBank, ExternalBinding, ExternalBuffers,
    HostedProcessorFactory, HostedProcessors, HostedRack, RecordingSinkError, RecordingSinks,
    RenderFormat, RenderPlan, RenderPlanError, buffers, effects, history, input_timeline, prepare,
    recording_session_key, sample_count,
};

impl fmt::Debug for BlockProcessor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Third-party processor trait objects deliberately do not expose implementation details.
        // Reporting their count still keeps exchange diagnostics useful without expanding the
        // real-time interface solely for formatting.
        formatter
            .debug_struct("BlockProcessor")
            .field("recorders", &self.recorders.len())
            .field("cycle_tracker", &self.cycle_tracker)
            .field("plan", &self.plan)
            .field("arena_samples", &self.arena.len())
            .field("source_buffers", &self.source_buffers.len())
            .field("sink_buffers", &self.sink_buffers.len())
            .field("source_latency_faults", &self.source_latency_faults)
            .field("source_timelines", &self.source_timelines)
            .field("sink_latency_faults", &self.sink_latency_faults)
            .field("sink_latency_routes", &self.sink_latency_routes)
            .field("has_meters", &self.meters.is_some())
            .field("dynamics_envelopes", &self.dynamics_envelopes.len())
            .field("dynamics_minimum_gains", &self.dynamics_minimum_gains.len())
            .field("dynamics_wet", &self.dynamics_wet.len())
            .field("effect_states", &self.effect_states.len())
            .field("device_bus_state", &self.device_bus_state.len())
            .field("device_bus_cursor", &self.device_bus_cursor)
            .field("hosted", &self.hosted)
            .field("controls", &self.controls)
            .field("control_scratch", &self.control_scratch)
            .field("sampled_control_epoch", &self.sampled_control_epoch)
            .field(
                "delay_bank",
                &self.delay_bank.as_ref().map(|bank| bank.state()),
            )
            .finish()
    }
}

impl BlockProcessor {
    /// # Errors
    /// Returns a storage reservation error before any audio ownership is published.
    pub fn new(plan: Arc<RenderPlan>) -> Result<Self, RenderPlanError> {
        Self::with_hosted(plan, Box::new([]))
    }

    pub(super) fn with_hosted(
        plan: Arc<RenderPlan>,
        hosted: HostedProcessors,
    ) -> Result<Self, RenderPlanError> {
        let effect_states = plan
            .effects
            .iter()
            .map(|binding| effects::State::new(&binding.design))
            .collect::<Result<Vec<_>, _>>()?
            .into_boxed_slice();
        let hosted = HostedRack::new(&plan, hosted)?;
        let arena = buffers::filled(plan.arena_samples, 0.0)?;
        let source_buffers = allocate_external_buffers(&plan.source_bindings, plan.format)?;
        let sink_buffers = allocate_external_buffers(&plan.sink_bindings, plan.format)?;
        let dynamics_envelopes = buffers::filled(plan.dynamics_state_count, 0.0)?;
        let dynamics_minimum_gains = buffers::filled(plan.dynamics_state_count, 1.0)?;
        let mut dynamics_wet = buffers::filled(plan.dynamics_state_count, 1.0)?;
        for binding in &plan.dynamics_bindings {
            dynamics_wet[binding.state_index] = match &binding.config {
                NodeConfig::Compressor(config) => f32::from(!config.bypass),
                NodeConfig::Limiter(config) => f32::from(!config.bypass),
                _ => unreachable!("only dynamics nodes own dynamics state"),
            };
        }
        let device_bus_state = buffers::filled(plan.device_bus_samples, 0.0)?;
        let controls = plan.controls.values();
        let control_scratch = controls.clone();
        let source_timelines =
            vec![input_timeline::State::default(); plan.source_bindings.len()].into_boxed_slice();
        let source_latency_faults = vec![false; plan.source_bindings.len()].into_boxed_slice();
        let sink_latency_faults = vec![false; plan.sink_bindings.len()].into_boxed_slice();
        let sink_latency_routes = plan
            .sink_bindings
            .iter()
            .map(|binding| plan.compensation.binding_routes(binding.node_id()))
            .collect();
        Ok(Self {
            source_timelines,
            source_latency_faults,
            sink_latency_faults,
            sink_latency_routes,
            recorders: Box::new([]),
            cycle_tracker: mixrelay_audio_io::process::CycleTracker::default(),
            plan,
            arena,
            source_buffers,
            sink_buffers,
            meters: None,
            dynamics_envelopes,
            dynamics_minimum_gains,
            dynamics_wet,
            effect_states,
            device_bus_state,
            device_bus_cursor: 0,
            hosted,
            controls,
            control_scratch,
            sampled_control_epoch: None,
            delay_bank: None,
        })
    }

    /// # Errors
    /// Returns a storage reservation error before any audio ownership is published.
    pub fn with_peak_meters(
        plan: Arc<RenderPlan>,
        meters: Arc<crate::PeakMeterBank>,
    ) -> Result<Self, RenderPlanError> {
        let mut processor = Self::new(plan)?;
        processor.meters = Some(meters);
        Ok(processor)
    }

    /// Prepares a graph containing provider nodes and installs their already-configured
    /// processor instances without doing plug-in work on the audio thread.
    ///
    /// # Errors
    ///
    /// Returns [`RenderPlanError`] when lowering or provider preparation fails.
    pub fn prepare_with_hosted_processors(
        execution: &ExecutionPlan,
        meters: Arc<crate::PeakMeterBank>,
        factory: &dyn HostedProcessorFactory,
    ) -> Result<Self, RenderPlanError> {
        let (plan, hosted_processors) = prepare::plan(execution, Some(factory))?;
        let mut processor = Self::with_hosted(Arc::new(plan), hosted_processors)?;
        processor.meters = Some(meters);
        processor.delay_bank = Some(Box::new(
            DelayBank::prepare(
                Arc::clone(processor.plan()),
                processor.plan().latency_observations(),
            )
            .map_err(RenderPlanError::CompensationPreparation)?,
        ));
        Ok(processor)
    }

    /// Transfers only route history; the caller retires the returned owner off the audio thread.
    pub(crate) fn replace_delay_bank(&mut self, bank: Box<DelayBank>) -> Option<Box<DelayBank>> {
        // The observer's first publication may repeat the bank prepared with this topology.
        // Retire that duplicate, not the warmed-up active history: otherwise startup causes
        // a second silence window after the first successful audio blocks.
        if self
            .delay_bank
            .as_ref()
            .is_some_and(|active| active.equivalent(&bank))
        {
            return Some(bank);
        }
        self.delay_bank.replace(bank)
    }

    /// Status of the derived alignment; unknown latency is never represented as known zero.
    #[must_use]
    pub fn compensation_state(&self) -> CompensationState {
        self.delay_bank
            .as_ref()
            .map_or(CompensationState::Unprepared, |bank| bank.state())
    }

    /// Audio-owner acknowledgement of the exact installed history observations.
    /// Returns false while compensation is pending, unprepared or priming.
    /// Atomic publication only; call after the processed cycle.
    #[must_use]
    pub fn acknowledge_content_latency(&self) -> bool {
        self.delay_bank
            .as_ref()
            .is_some_and(|bank| bank.acknowledge_content_latency())
    }

    /// Block-local transfer and history readiness for one external path binding.
    /// The reporter combines only the source/sink bindings contributing to its destination.
    #[must_use]
    pub fn binding_content_latency_ready(&self, id: &str, input: bool) -> bool {
        let (bindings, faults, routes): (_, _, &[Vec<usize>]) = if input {
            (
                &self.plan.source_bindings,
                &self.source_latency_faults,
                &[][..],
            )
        } else {
            (
                &self.plan.sink_bindings,
                &self.sink_latency_faults,
                &self.sink_latency_routes,
            )
        };
        bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| {
                id == binding.node_id().as_str()
                    || id.strip_suffix(if input { "/capture" } else { "/render" })
                        == Some(binding.node_id().as_str())
            })
            .any(|(index, _)| {
                !faults[index]
                    && self.delay_bank.as_ref().is_some_and(|bank| {
                        bank.content_ready_for_routes(if input { &[] } else { &routes[index] })
                    })
            })
    }

    pub(crate) fn can_adopt_hosted(&self, previous: &Self) -> bool {
        self.hosted.can_adopt(&previous.hosted)
    }

    pub(crate) fn set_hosted_active(&mut self, active: bool) {
        self.hosted.set_active(active);
    }

    pub(crate) fn retain_history(&mut self, previous: &mut Self) {
        history::retain(self, previous);
    }

    pub(crate) fn retain_device_bus_state(&mut self, previous: &Self) {
        if self.plan.format != previous.plan.format {
            return;
        }
        self.device_bus_cursor = previous.device_bus_cursor;
        for binding in &self.plan.device_bus_bindings {
            let Ok(index) = previous
                .plan
                .device_bus_bindings
                .binary_search_by(|candidate| candidate.node_id.cmp(&binding.node_id))
            else {
                continue;
            };
            let source = &previous.plan.device_bus_bindings[index];
            if source.samples.len() == binding.samples.len()
                && source.input_path == binding.input_path
            {
                self.device_bus_state[binding.samples.clone()]
                    .copy_from_slice(&previous.device_bus_state[source.samples.clone()]);
            }
        }
    }

    /// Moves complete processor ownership at activation; no allocation, lock, native call,
    /// or worker destruction occurs on the audio thread. Retired placeholders drop later.
    pub(crate) fn adopt_hosted(&mut self, previous: &mut Self) {
        self.hosted.adopt(&mut previous.hosted);
    }

    /// Attaches app-owned recording sinks while preparing an inactive processor.
    /// The app's control plane decides when a sink records.
    /// # Errors
    /// Returns the first sink that could not be prepared; no sink is attached then.
    pub fn attach_recorders(
        &mut self,
        sinks: &dyn RecordingSinks,
    ) -> Result<(), RecordingSinkError> {
        self.recorders = self
            .plan
            .recorder_bindings
            .iter()
            .map(|binding| {
                sinks.prepare(
                    &recording_session_key(&binding.node_id, binding.channels),
                    binding.channels,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_boxed_slice();
        Ok(())
    }

    #[must_use]
    pub const fn plan(&self) -> &Arc<RenderPlan> {
        &self.plan
    }
}

fn allocate_external_buffers(
    bindings: &[ExternalBinding],
    format: RenderFormat,
) -> Result<ExternalBuffers, RenderPlanError> {
    let mut result = buffers::reserve(bindings.len())?;
    for binding in bindings {
        result.push(buffers::filled(
            sample_count(format.block_frames as usize, binding.channels),
            0.0,
        )?);
    }
    Ok(result.into_boxed_slice())
}
