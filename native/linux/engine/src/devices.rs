//! Physical/source/sink I/O ownership, prepared off the real-time thread.
//! Graph gains are DSP parameters, never part of native stream identity.
use crate::processing::failure::{Failure, FailureMailbox};
use mixrelay_audio_graph::{AudioDeviceInputConfig, AudioDeviceOutputConfig, RenderFormat};
use mixrelay_audio_io::{
    DeviceDirection, DeviceSelector, HostAudioBackendError,
    process::{ProcessReport, TransferOutcome},
};
use mixrelay_engine_core::resource_requirements::DeviceRequirements;
use mixrelay_linux_audio_io::PipeWireTarget;
use mixrelay_linux_pipewire::{NativeStream, OutputRelease, StreamOwners};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

mod open;
mod reconciliation;
#[cfg(test)]
mod tests;
mod watch;

pub(crate) use watch::DeviceWatch;

#[derive(Default)]
pub(crate) struct DeviceStreams {
    owners: StreamOwners,
    pub(crate) generation: u64,
    inputs: BTreeMap<String, ObservedStream>,
    outputs: BTreeMap<String, ObservedStream>,
    format: Option<RenderFormat>,
    #[cfg(test)]
    automatic_outputs: bool,
}

struct ObservedStream {
    // None is a preallocated transfer slot, never an acquired native owner.
    stream: Option<NativeStream>,
    // Identity of this Arc is the acquisition lease. Recovery retains it; a
    // removed/re-added source gets a new cell even when its binding ID is reused.
    connected: Arc<AtomicBool>,
    // Shared selection identity changes only after the RT owner accepts a swap.
    selected: Arc<DeviceSelection>,
    serial: u64,
    replaces: Option<u64>,
}

struct DeviceSelection {
    node_id: mixrelay_audio_graph::NodeId,
    serial: AtomicU64,
    enabled: AtomicBool,
}

impl DeviceSelection {
    fn new(id: &str, serial: u64) -> Self {
        Self {
            node_id: id.into(),
            serial: AtomicU64::new(serial),
            // Preparation cannot request recovery before the audio worker
            // observes the executing topology's node control gate.
            enabled: AtomicBool::new(false),
        }
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
}

/// One direction's device request. Inputs and outputs differ only in these facts.
trait DeviceRequest: Clone + PartialEq {
    const DIRECTION: DeviceDirection;
    fn selector(&self) -> &DeviceSelector;
    fn channels(&self) -> u16;

    /// Whether the request names the system default of its own direction.
    fn follows_default(&self) -> bool {
        matches!(
            (self.selector(), Self::DIRECTION),
            (DeviceSelector::DefaultInput, DeviceDirection::Input)
                | (DeviceSelector::DefaultOutput, DeviceDirection::Output)
        )
    }

    fn resolve<'a>(
        &self,
        targets: &'a [PipeWireTarget],
    ) -> Result<&'a PipeWireTarget, HostAudioBackendError> {
        mixrelay_linux_audio_io::resolve_device_target(
            targets,
            self.selector(),
            Self::DIRECTION,
            self.channels(),
        )
    }
}

impl DeviceRequest for AudioDeviceInputConfig {
    const DIRECTION: DeviceDirection = DeviceDirection::Input;
    fn selector(&self) -> &DeviceSelector {
        &self.selector
    }
    fn channels(&self) -> u16 {
        self.output_channels
    }
}

impl DeviceRequest for AudioDeviceOutputConfig {
    const DIRECTION: DeviceDirection = DeviceDirection::Output;
    fn selector(&self) -> &DeviceSelector {
        &self.selector
    }
    fn channels(&self) -> u16 {
        self.input_channels
    }
}

impl ObservedStream {
    fn new(id: &str, stream: NativeStream, serial: u64) -> Self {
        Self {
            stream: Some(stream),
            connected: Arc::new(AtomicBool::new(true)),
            selected: Arc::new(DeviceSelection::new(id, serial)),
            serial,
            replaces: None,
        }
    }

    fn matches_recovery(&self, next: &Self) -> bool {
        Arc::ptr_eq(&self.connected, &next.connected) && Arc::ptr_eq(&self.selected, &next.selected)
    }

    /// A disconnected owner accepts any recovered stream. A healthy one accepts only a
    /// prepared default selection whose expected serial still matches the active owner.
    fn accepts_replacement(&self, next: &Self) -> bool {
        self.selected.enabled()
            && (self
                .stream
                .as_ref()
                .is_some_and(|stream| !stream.connected())
                || next.replaces.is_some_and(|expected| {
                    expected == self.selected.serial.load(Ordering::Acquire)
                        && next.serial != expected
                }))
    }

    fn ready(&self) -> bool {
        self.stream
            .as_ref()
            .is_some_and(|stream| stream.connected() && stream.ready())
    }
}

impl DeviceStreams {
    pub(crate) fn bind_inputs(
        &mut self,
        inputs: &mixrelay_linux_pipewire::GraphInputs,
    ) -> Result<(), String> {
        for stream in self
            .inputs
            .values_mut()
            .filter_map(|entry| entry.stream.as_mut())
        {
            stream.bind_inputs(inputs)?;
        }
        Ok(())
    }

    pub(crate) fn initialize_controls(&self, document: &mixrelay_audio_graph::GraphDocument) {
        // Vacant staged slots borrow live selection leases. Their controls belong
        // to the executing plan until publication and activation succeed.
        for owner in self.slots() {
            if owner.stream.is_none() {
                continue;
            }
            let enabled = document
                .nodes
                .iter()
                .find(|node| node.id == owner.selected.node_id)
                .is_some_and(|node| node.control.enabled);
            owner.selected.enabled.store(enabled, Ordering::Release);
        }
    }

    pub(crate) fn observe_controls(&self, controls: &mixrelay_render::RuntimeControlBank) {
        for owner in self.slots() {
            let enabled = controls
                .snapshot(&owner.selected.node_id)
                .is_some_and(|(control, _)| control.enabled);
            owner.selected.enabled.store(enabled, Ordering::Release);
        }
    }

    pub(crate) fn discard_inputs(&mut self) {
        for stream in self
            .inputs
            .values_mut()
            .filter_map(|entry| entry.stream.as_mut())
        {
            stream.discard_input();
        }
    }

    pub(crate) fn finish_input_block(&mut self) {
        for stream in self
            .inputs
            .values_mut()
            .filter_map(|entry| entry.stream.as_mut())
        {
            stream.finish_capture_block();
        }
    }

    pub(crate) fn shares_clock(&self, id: u32, rate: u32) -> bool {
        self.slots().all(|entry| {
            entry
                .stream
                .as_ref()
                .is_some_and(|stream| stream.shares_clock(id, rate))
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.outputs.is_empty()
    }

    fn slots(&self) -> impl Iterator<Item = &ObservedStream> {
        self.inputs.values().chain(self.outputs.values())
    }

    /// Read-only health and selection cells for the recovery worker.
    pub(crate) fn watch(&self, spec: DeviceRequirements) -> DeviceWatch {
        DeviceWatch::of(self, spec)
    }

    /// RT-only swaps existing slots. Keep original health cells so the control
    /// watcher follows the replacement. Healthy owners change only for a prepared
    /// default selection whose expected serial still matches the active owner.
    pub(crate) fn replace_disconnected(&mut self, next: &mut Self) {
        if self.generation != next.generation {
            return;
        }
        for (current, replacements) in [
            (&mut self.inputs, &mut next.inputs),
            (&mut self.outputs, &mut next.outputs),
        ] {
            for (id, replacement) in replacements {
                if let Some(value) = current.get_mut(id)
                    && value.matches_recovery(replacement)
                    && value.accepts_replacement(replacement)
                    && let Some(stream) = &mut replacement.stream
                {
                    // Input prepared during a retry must not replay queued old
                    // microphone audio. Native output modes deliberately ignore this.
                    stream.discard_input();
                    std::mem::swap(&mut value.stream, &mut replacement.stream);
                    std::mem::swap(&mut value.serial, &mut replacement.serial);
                    value.selected.serial.store(value.serial, Ordering::Release);
                    value.connected.store(
                        value.stream.as_ref().is_some_and(NativeStream::connected),
                        Ordering::Release,
                    );
                }
            }
        }
    }

    pub(crate) fn read(&mut self, node: &str, samples: &mut [f32]) -> TransferOutcome {
        samples.fill(0.0);
        self.inputs
            .get_mut(node)
            .and_then(|entry| entry.stream.as_mut())
            .map_or(TransferOutcome::complete(0), |stream| {
                if !stream.connected() {
                    return disconnected_transfer();
                }
                stream.read_capture(samples)
            })
    }

    pub(crate) fn write(&mut self, node: &str, samples: &[f32]) -> TransferOutcome {
        self.outputs
            .get_mut(node)
            .and_then(|entry| entry.stream.as_mut())
            .map_or(TransferOutcome::complete(0), |stream| {
                if !stream.connected() {
                    return disconnected_transfer();
                }
                TransferOutcome::complete(stream.write(samples))
            })
    }

    /// Releases canonical physical output owners once per native cycle, including
    /// OFF nodes whose live transport must still drain without bypassing DSP gates.
    pub(crate) fn trigger_outputs(&mut self, failures: &FailureMailbox) {
        for stream in self
            .outputs
            .values_mut()
            .filter_map(|entry| entry.stream.as_mut())
        {
            if let OutputRelease::Rejected(code) = stream.trigger_output() {
                failures.publish_native(Failure::OutputReleaseRejected, code);
            }
        }
    }

    pub(crate) fn take_report(&mut self) -> ProcessReport {
        let mut report = ProcessReport::default();
        for entry in self.inputs.values_mut().chain(self.outputs.values_mut()) {
            let Some(stream) = &mut entry.stream else {
                continue;
            };
            entry.connected.store(stream.connected(), Ordering::Release);
            let next = stream.take_report();
            report.input_starvations += next.input_starvations;
            report.output_overflows += next.output_overflows;
            report.discontinuities += next.discontinuities;
        }
        report
    }
}

fn disconnected_transfer() -> TransferOutcome {
    TransferOutcome {
        frames: 0,
        discontinuity: true,
        ..TransferOutcome::complete(0)
    }
}

#[cfg(all(test, feature = "native-lab"))]
impl DeviceStreams {
    pub(crate) fn input_owner(&self, id: &str) -> Option<usize> {
        self.inputs
            .get(id)?
            .stream
            .as_ref()
            .map(NativeStream::allocation_id)
    }

    /// Observes acquired physical input/output owners without changing report baselines.
    pub(crate) fn transfer_snapshot(&self) -> mixrelay_linux_pipewire::TransferSnapshot {
        let mut snapshot = mixrelay_linux_pipewire::TransferSnapshot::default();
        for stream in self.slots().filter_map(|entry| entry.stream.as_ref()) {
            snapshot.include(stream);
        }
        snapshot
    }
}
