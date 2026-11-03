//! Engine-side processor endpoint, fixed IPC latency and worker lifetime ownership.

use crate::{
    AudioLayout,
    catalog::{CatalogEntry, selected_ports},
    control::{HostedProcessorControl, ProcessorControlRegistration},
    dry_delay,
    shared_transport::{self, ParentAudio},
    timing, worker_configuration,
    worker_paths::worker_paths,
    worker_process::{
        Timeouts, WorkerProcess, cleanup_worker_config_path, cleanup_worker_paths,
        spawn_processor_worker,
    },
};
use mixrelay_audio_engine::{HostedAudioProcessor, HostedLatencySource};
use mixrelay_audio_graph::{ProviderNodeConfig, RenderFormat};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

pub(super) struct HostedExternalProcessor {
    pub(super) worker: Option<WorkerProcess>,
    pub(super) shared: Option<ParentAudio>,
    pub(super) config_path: PathBuf,
    pub(super) layout: AudioLayout,
    pub(super) dry_delay: dry_delay::DryDelay,
    pub(super) next_sequence: u64,
    pub(super) timing: timing::TimingProbe,
    pub(super) control: Option<Arc<HostedProcessorControl>>,
    pub(super) registration: Option<ProcessorControlRegistration>,
}

pub(super) struct NativeInstance {
    pub(super) id: crate::instance_id::InstanceId,
    pub(super) clock: Arc<dyn mixrelay_time::MonotonicClock>,
}

impl HostedExternalProcessor {
    /// Starts the worker and returns the processor with the control handle to register.
    pub(super) fn new(
        entry: &CatalogEntry,
        config: &ProviderNodeConfig,
        format: RenderFormat,
        state_store: Option<&mixrelay_processor_state::StateStore>,
        instance: NativeInstance,
    ) -> Result<(Self, Arc<HostedProcessorControl>), String> {
        let selected = selected_ports(&entry.descriptor, config)?;
        let input_channels = usize::from(
            selected
                .total_input_channels()
                .map_err(|error| error.to_string())?,
        );
        let output_channels = usize::from(
            selected
                .total_output_channels()
                .map_err(|error| error.to_string())?,
        );
        let block_frames = format.block_frames as usize;
        let dry_delay = dry_delay::DryDelay::for_ports(&selected, block_frames)
            .map_err(|error| error.to_string())?;
        let paths = worker_paths().map_err(|error| error.to_string())?;
        let shared_path = paths.shared().to_path_buf();
        let config_path = paths.config().to_path_buf();
        let layout = AudioLayout {
            inputs: input_channels,
            outputs: output_channels,
            block_frames,
        };
        let (shared, control) = shared_transport::create_owned(&paths, layout, instance.id)?;
        let control = Arc::new(
            HostedProcessorControl::native(control, instance.clock)
                .with_format(entry.rack.locator.format),
        );
        if let Err(error) = worker_configuration::write_worker_config(
            &config_path,
            format.sample_rate,
            &config.parameters,
            state_store,
            &selected,
        ) {
            drop(control);
            drop(shared);
            cleanup_worker_paths(&shared_path, &config_path);
            return Err(error);
        }
        let child = match spawn_processor_worker(entry, &shared_path, &config_path) {
            Ok(child) => child,
            Err(error) => {
                drop(control);
                drop(shared);
                cleanup_worker_paths(&shared_path, &config_path);
                return Err(error);
            }
        };
        let worker =
            match WorkerProcess::start(child, Arc::clone(&control), &shared, Timeouts::default()) {
                Ok(worker) => worker,
                Err(error) => {
                    drop(control);
                    drop(shared);
                    cleanup_worker_paths(&shared_path, &config_path);
                    return Err(error);
                }
            };
        control.own_transport_file(shared_path.clone());
        let processor = Self {
            worker: Some(worker),
            shared: Some(shared),
            config_path,
            layout,
            next_sequence: 1,
            dry_delay,
            control: Some(Arc::clone(&control)),
            timing: timing::TimingProbe::default(),
            registration: None,
        };
        Ok((processor, control))
    }
}

impl HostedAudioProcessor for HostedExternalProcessor {
    fn suspend_processing(&mut self) {
        if let Some(shared) = &mut self.shared {
            shared.discard_history();
        }
        self.dry_delay.reset();
    }

    fn set_active(&mut self, active: bool) {
        if let Some(control) = &self.control {
            control.active_owner.store(active, Ordering::Release);
        }
    }
    fn reuse_identity(&self) -> Option<&mixrelay_audio_engine::HostedProcessorIdentity> {
        self.control
            .as_ref()
            .map(|control| &control.adoption_identity)
    }

    fn latency_source(&self) -> Option<Arc<dyn HostedLatencySource>> {
        self.control.as_ref().map(|control| {
            // Retain this prepared worker's mapping across plan adoption and retirement.
            let source: Arc<dyn HostedLatencySource> =
                Arc::<HostedProcessorControl>::clone(control);
            source
        })
    }

    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        self.set_active(true);
        let Some(input_samples) = self.layout.inputs.checked_mul(frames) else {
            return false;
        };
        let Some(output_samples) = self.layout.outputs.checked_mul(frames) else {
            return false;
        };
        if frames == 0
            || frames > self.layout.block_frames
            || input.len() < input_samples
            || output.len() < output_samples
        {
            return false;
        }
        // The mapping is released only by `Drop`; without one the block is not processed.
        let Some(shared) = self.shared.as_mut() else {
            return false;
        };
        let received = shared.receive(output, frames, |sequence| {
            if let Some(control) = &self.control {
                self.timing
                    .received(sequence, &control.timing, control.clock.as_ref());
            }
        });
        if !received {
            shared
                .header()
                .process_faults
                .record(crate::process_fault::FaultKind::OutputDeadlineMissed);
        }
        if shared.submit(self.next_sequence, input, frames, || {
            if let Some(control) = &self.control {
                self.timing
                    .submitted(self.next_sequence, &control.timing, control.clock.as_ref());
            }
        }) {
            self.next_sequence = self.next_sequence.saturating_add(1);
        } else {
            shared
                .header()
                .process_faults
                .record(crate::process_fault::FaultKind::InputBackpressure);
        }
        // A missed deadline must not jump ahead to current input. Keep the dry path
        // aligned to the prepared IPC delay even while healthy output is available.
        // Intrinsic plug-in latency is a separate compensation boundary.
        self.dry_delay
            .advance(input, (!received).then_some(output), frames)
    }
}

impl Drop for HostedExternalProcessor {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.header().shutdown.store(1, Ordering::Release);
        }
        drop(self.worker.take());
        // A retained latency reader keeps the control mapping and its transport-file cleanup
        // guard alive. Release the parent mapping before this processor's control Arc; the final
        // control handle unlinks the path only after its mailbox mapping has closed.
        drop(self.registration.take());
        drop(self.shared.take());
        drop(self.control.take());
        cleanup_worker_config_path(&self.config_path);
    }
}
