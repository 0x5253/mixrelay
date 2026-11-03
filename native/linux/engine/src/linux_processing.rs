//! Native graph-cycle ownership, DSP dispatch and runtime resource activation.
use super::{
    DeviceStreams, EndpointSet, LinuxVirtualEndpointBackend, PipeWireCaptureSet, clock,
    driver_monitor,
};
use crate::processing_failure::{Failure, FailureMailbox};
use mixrelay_audio_engine::{BlockProcessor, ExternalBinding, ExternalIo, ProcessorExchange};
use mixrelay_audio_graph::{FROM_OS_PORT_ID, TO_OS_PORT_ID};
use mixrelay_audio_io::{
    process::{CycleBlocks, ProcessCycle, ProcessReport, TransferOutcome},
    recovery::RecoveryConsumer,
    session::ClockAuthority,
};
use mixrelay_engine::session::AdapterRuntimeStatus;
use mixrelay_pipewire::{DriverCycle, DriverFormat, DriverProcess, GraphDriver};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("could not acquire PipeWire processing ownership: {0}")]
    Ownership(&'static str),
    #[error("{0} recovery already has an audio consumer")]
    RecoveryConsumer(&'static str),
    #[error("could not prepare PipeWire graph driver format: {0}")]
    DriverFormat(#[source] mixrelay_pipewire::DriverError),
    #[error("could not prepare PipeWire graph timeline: {0}")]
    Timeline(#[source] mixrelay_audio_io::process::CycleError),
    #[error("could not start PipeWire graph driver: {0}")]
    Driver(#[source] mixrelay_pipewire::DriverError),
    #[error(transparent)]
    Runtime(#[from] driver_monitor::Error),
}

pub(super) struct LinuxProcessState<'a> {
    pub(super) processor: Box<BlockProcessor>,
    pub(super) exchange: Arc<ProcessorExchange>,
    pub(super) endpoint_backend: &'a LinuxVirtualEndpointBackend,
    pub(super) application_recovery: RecoveryConsumer<'a, PipeWireCaptureSet>,
    pub(super) device_recovery: RecoveryConsumer<'a, DeviceStreams>,
    pub(super) endpoints: Box<EndpointSet>,
    pub(super) captures: Box<PipeWireCaptureSet>,
    pub(super) devices: Box<DeviceStreams>,
    pub(super) status: Arc<AdapterRuntimeStatus>,
    pub(super) timeline: clock::GraphTimeline,
    pub(super) clock_diagnostics: Arc<clock::DiscontinuityMailbox>,
    pub(super) failure: Arc<FailureMailbox>,
    pub(super) resource_failures: Arc<FailureMailbox>,
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
    let application_recovery = endpoint_backend
        .application_recovery
        .try_consumer()
        .ok_or(WorkerError::RecoveryConsumer("application"))?;
    let device_recovery = endpoint_backend
        .device_recovery
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
    fn prepare_inputs(
        &mut self,
        inputs: &mixrelay_pipewire::GraphInputs,
    ) -> Result<(), mixrelay_pipewire::DriverError> {
        self.endpoints
            .bind_inputs(inputs)
            .map_err(mixrelay_pipewire::DriverError::Input)?;
        self.captures
            .bind_inputs(inputs)
            .map_err(mixrelay_pipewire::DriverError::Input)?;
        self.devices
            .bind_inputs(inputs)
            .map_err(mixrelay_pipewire::DriverError::Input)?;
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
        self.endpoint_backend.observation.observe(&self.endpoints);
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
            mixrelay_audio_io::session::SessionFormat {
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

struct LinuxExternalIo<'a> {
    endpoints: &'a mut EndpointSet,
    application_captures: &'a mut PipeWireCaptureSet,
    devices: &'a mut DeviceStreams,
}

impl ExternalIo for LinuxExternalIo<'_> {
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        _cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if binding.port_id().as_str() == FROM_OS_PORT_ID {
            return self.endpoints.read(binding.node_id().as_str(), destination);
        } else if binding.port_id().as_str() == mixrelay_audio_graph::AUDIO_PORT_ID {
            if !self
                .application_captures
                .contains(binding.node_id().as_str())
            {
                return self.devices.read(binding.node_id().as_str(), destination);
            }
            return self
                .application_captures
                .read(binding.node_id().as_str(), destination);
        }
        TransferOutcome::complete(0)
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        _cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if binding.port_id().as_str() == TO_OS_PORT_ID {
            return self.endpoints.write(binding.node_id().as_str(), source);
        }
        if binding.port_id().as_str() == mixrelay_audio_graph::AUDIO_PORT_ID {
            return self.devices.write(binding.node_id().as_str(), source);
        }
        TransferOutcome::complete(0)
    }
}

#[cfg(test)]
#[path = "linux_processing/tests.rs"]
mod tests;
