//! Native graph-cycle ownership, DSP dispatch and runtime resource activation.
use crate::{
    applications::CaptureSet, backend::LinuxVirtualEndpointBackend, devices::DeviceStreams,
    endpoints::EndpointSet,
};
use external_io::LinuxExternalIo;
use failure::{Failure, FailureMailbox};
use mixrelay_audio_io::{
    process::{CycleBlocks, ProcessCycle, ProcessReport},
    recovery::RecoveryConsumer,
    session::{ClockAuthority, SessionFormat},
};
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use mixrelay_linux_pipewire::{
    DriverCycle, DriverError, DriverFormat, DriverProcess, GraphDriver, GraphInputs,
};
use mixrelay_render::{BlockProcessor, ProcessorExchange};
use std::sync::Arc;

pub(crate) mod clock;
mod driver_monitor;
mod external_io;
pub(crate) mod failure;
#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("could not acquire PipeWire processing ownership: {0}")]
    Ownership(&'static str),
    #[error("{0} recovery already has an audio consumer")]
    RecoveryConsumer(&'static str),
    #[error("could not prepare PipeWire graph driver format: {0}")]
    DriverFormat(#[source] DriverError),
    #[error("could not prepare PipeWire graph timeline: {0}")]
    Timeline(#[source] mixrelay_audio_io::process::CycleError),
    #[error("could not start PipeWire graph driver: {0}")]
    Driver(#[source] DriverError),
    #[error(transparent)]
    Runtime(#[from] driver_monitor::Error),
}

/// Everything the native callback owns while the graph driver runs.
pub(crate) struct LinuxProcessState<'a> {
    pub(crate) processor: Box<BlockProcessor>,
    pub(crate) exchange: Arc<ProcessorExchange>,
    pub(crate) endpoint_backend: &'a LinuxVirtualEndpointBackend,
    pub(crate) application_recovery: RecoveryConsumer<'a, CaptureSet>,
    pub(crate) device_recovery: RecoveryConsumer<'a, DeviceStreams>,
    pub(crate) endpoints: Box<EndpointSet>,
    pub(crate) captures: Box<CaptureSet>,
    pub(crate) devices: Box<DeviceStreams>,
    pub(crate) status: Arc<AdapterRuntimeStatus>,
    pub(crate) timeline: clock::GraphTimeline,
    pub(crate) clock_diagnostics: Arc<clock::DiscontinuityMailbox>,
    pub(crate) failure: Arc<FailureMailbox>,
    pub(crate) resource_failures: Arc<FailureMailbox>,
}

/// Owns native callback processing until stop and complete native drain.
/// # Errors
/// Returns failed preparation or a terminal native processing failure.
pub fn run_audio_worker(
    processor: Box<BlockProcessor>,
    exchange: Arc<ProcessorExchange>,
    endpoint_backend: &LinuxVirtualEndpointBackend,
    status: &Arc<AdapterRuntimeStatus>,
) -> Result<(), WorkerError> {
    let _lease = status.begin_processing().map_err(WorkerError::Ownership)?;
    let (applications, devices) = endpoint_backend.recovery_mailboxes();
    let application_recovery = applications
        .try_consumer()
        .ok_or(WorkerError::RecoveryConsumer("application"))?;
    let device_recovery = devices
        .try_consumer()
        .ok_or(WorkerError::RecoveryConsumer("device"))?;
    let initial_format = processor.plan().format();
    let driver_format = DriverFormat::new(initial_format.sample_rate, initial_format.block_frames)
        .map_err(WorkerError::DriverFormat)?;
    let timeline =
        clock::GraphTimeline::new(initial_format.sample_rate).map_err(WorkerError::Timeline)?;
    let clock_diagnostics = Arc::new(clock::DiscontinuityMailbox::default());
    let failure = Arc::new(FailureMailbox::default());
    let resource_failures = Arc::new(FailureMailbox::default());
    let context = LinuxProcessState {
        processor,
        exchange,
        endpoint_backend,
        application_recovery,
        device_recovery,
        endpoints: Box::default(),
        captures: Box::default(),
        devices: Box::default(),
        status: status.clone(),
        timeline,
        clock_diagnostics: Arc::clone(&clock_diagnostics),
        failure: Arc::clone(&failure),
        resource_failures: Arc::clone(&resource_failures),
    };
    let driver = GraphDriver::open(context, driver_format).map_err(WorkerError::Driver)?;
    driver_monitor::run(
        driver,
        status,
        &clock_diagnostics,
        &failure,
        &resource_failures,
    )?;
    Ok(())
}

impl DriverProcess for LinuxProcessState<'_> {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        self.endpoints
            .bind_inputs(inputs)
            .map_err(DriverError::Input)?;
        self.captures
            .bind_inputs(inputs)
            .map_err(DriverError::Input)?;
        self.devices
            .bind_inputs(inputs)
            .map_err(DriverError::Input)?;
        self.endpoint_backend.register_inputs(inputs)
    }

    fn process(&mut self, native: &DriverCycle) {
        if self.status.stop_requested() || self.failure.read().is_some() || self.status.checkpoint()
        {
            return;
        }
        if let Err(cause) = self.process_cycle(native) {
            self.failure.publish(cause);
        }
    }
}

impl LinuxProcessState<'_> {
    fn process_cycle(&mut self, native: &DriverCycle) -> Result<(), Failure> {
        let format = self.activate_pending_resources()?;
        self.endpoint_backend.observation().observe(&self.endpoints);
        let cycle = self.timeline.map(native)?;
        if let Some(event) = self.timeline.take_discontinuity() {
            self.clock_diagnostics.publish(event);
        }
        self.status
            .observe_clock(ClockAuthority::AudioServer, false);
        if cycle.frames == 0 {
            // A valid native iteration still owes the manual output dependency,
            // even when duration mapping has not accumulated one PCM frame yet.
            process_blocks_and_release(self, None, Self::process_block, Self::trigger_outputs)?;
            return Ok(());
        }
        let blocks = CycleBlocks::new(
            cycle,
            SessionFormat {
                sample_rate: format.sample_rate,
                block_frames: format.block_frames,
            },
        )
        .map_err(Failure::from)?;
        if cycle.discontinuity && cycle.clock.epoch > 1 {
            self.status.record_process(ProcessReport {
                discontinuities: 1,
                ..ProcessReport::default()
            });
        }
        process_blocks_and_release(
            self,
            Some(blocks),
            Self::process_block,
            Self::trigger_outputs,
        )?;
        // A node.group preference is not proof of a shared driver. Report only
        // actual native callback IDs after every currently active owner agrees.
        let synchronized = self.endpoints.shares_clock(native.clock_id, native.rate)
            && self.captures.shares_clock(native.clock_id, native.rate)
            && self.devices.shares_clock(native.clock_id, native.rate);
        self.status
            .observe_clock(ClockAuthority::AudioServer, synchronized);
        Ok(())
    }

    /// Swaps in the plan and native owners staged for the next revision and returns the
    /// render format now in effect. A changed format restarts the native timeline.
    fn activate_pending_resources(
        &mut self,
    ) -> Result<mixrelay_audio_graph::RenderFormat, Failure> {
        let previous_format = self.processor.plan().format();
        let activated =
            match self
                .exchange
                .activate_pending_configured(&mut self.processor, |revision| {
                    self.endpoint_backend.activate_pending(
                        revision,
                        &mut self.endpoints,
                        &mut self.captures,
                        &mut self.devices,
                    )
                }) {
                Ok(activated) => activated,
                // Control reclamation is bounded backpressure, not a failed audio cycle.
                Err(Failure::RetirementPending) => false,
                Err(cause) => {
                    self.resource_failures.publish(cause);
                    false
                }
            };
        let format = self.processor.plan().format();
        if activated && previous_format != format {
            self.timeline = clock::GraphTimeline::new(format.sample_rate).map_err(Failure::from)?;
            self.status.activate_format(format);
        }
        Ok(format)
    }

    fn trigger_outputs(&mut self) {
        self.endpoints.trigger_outputs(&self.resource_failures);
        self.devices.trigger_outputs(&self.resource_failures);
    }

    fn process_block(&mut self, cycle: ProcessCycle) -> Result<(), Failure> {
        self.devices
            .observe_controls(self.processor.plan().controls());
        self.application_recovery
            .activate(self.captures.generation, |next| {
                self.captures.replace(next);
            });
        self.device_recovery
            .activate(self.devices.generation, |next| {
                self.devices.replace_disconnected(next);
            });
        let report = self
            .processor
            .process_external_cycle(
                cycle,
                &mut LinuxExternalIo {
                    endpoints: &mut self.endpoints,
                    application_captures: &mut self.captures,
                    devices: &mut self.devices,
                },
            )
            .map_err(Failure::from)?;
        self.status
            .record_process(self.endpoints.finish_block(cycle.frames)?);
        self.captures.finish_block(cycle.frames);
        self.devices.finish_input_block();
        let Some(report) = report else {
            return Ok(());
        };
        self.status.record_process(report);
        self.status.record_process(self.devices.take_report());
        self.status.record_process(self.endpoints.take_report());
        self.status.record_process(self.captures.take_report());
        self.status.observe_application_state(self.captures.state());
        Ok(())
    }
}

// Static callbacks keep the native-cycle sequencing testable without allocating
// or introducing a second dispatch/ownership path in the real-time process.
fn process_blocks_and_release<Owner>(
    owner: &mut Owner,
    blocks: Option<CycleBlocks>,
    mut process_block: impl FnMut(&mut Owner, ProcessCycle) -> Result<(), Failure>,
    release_outputs: impl FnOnce(&mut Owner),
) -> Result<(), Failure> {
    for block in blocks.into_iter().flatten() {
        process_block(owner, block)?;
    }
    release_outputs(owner);
    Ok(())
}
