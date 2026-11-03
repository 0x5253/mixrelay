//! Physical/source/sink I/O ownership, prepared off the real-time thread.
//! Graph gains are DSP parameters, never part of native stream identity.
use super::native::{NativeStream, StreamRole};
#[cfg(test)]
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, GraphDocument, NodeConfig,
};
use mixrelay_audio_io::{
    DeviceDirection,
    process::{ProcessReport, TransferOutcome},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use mixrelay_engine_core::resource_requirements::DeviceRequirements;

#[path = "linux_device_reconciliation.rs"]
mod reconciliation;

#[derive(Default)]
pub(super) struct DeviceStreams {
    owners: super::native::StreamOwners,
    pub(super) generation: u64,
    inputs: BTreeMap<String, ObservedStream>,
    outputs: BTreeMap<String, ObservedStream>,
    format: Option<mixrelay_audio_graph::RenderFormat>,
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
}

fn target_serial(target: &mixrelay_linux_audio_io::PipeWireTarget) -> Result<u64, String> {
    target
        .serial
        .parse()
        .map_err(|_| "invalid PipeWire serial".to_owned())
}

/// Startup is atomic; recovery may admit independently returning devices and
/// always carries cancellation. These are not independent boolean choices.
#[derive(Clone, Copy, Debug)]
enum OpenPolicy<'a> {
    Initial,
    Recovery(&'a AtomicBool),
    #[cfg(test)]
    StandalonePublisher,
    #[cfg(test)]
    StandaloneRecovery(&'a AtomicBool),
}

impl OpenPolicy<'_> {
    fn check_cancelled(self) -> Result<(), String> {
        let stopping = match self {
            Self::Recovery(stopping) => Some(stopping),
            #[cfg(test)]
            Self::StandaloneRecovery(stopping) => Some(stopping),
            _ => None,
        };
        if stopping.is_some_and(|stopping| stopping.load(Ordering::Acquire)) {
            Err("device recovery cancelled".to_owned())
        } else {
            Ok(())
        }
    }

    const fn allows_partial(self) -> bool {
        match self {
            Self::Recovery(_) => true,
            #[cfg(test)]
            Self::StandaloneRecovery(_) => true,
            _ => false,
        }
    }

    fn output_role(self, target: &str) -> StreamRole<'_> {
        match self {
            Self::Initial | Self::Recovery(_) => StreamRole::DeviceOutput(target),
            #[cfg(test)]
            Self::StandalonePublisher | Self::StandaloneRecovery(_) => {
                StreamRole::AutomaticDeviceOutput(target)
            }
        }
    }

    #[cfg(test)]
    const fn automatic_outputs(self) -> bool {
        matches!(
            self,
            Self::StandalonePublisher | Self::StandaloneRecovery(_)
        )
    }
}
impl DeviceStreams {
    pub(super) fn bind_inputs(
        &mut self,
        inputs: &mixrelay_linux_pipewire::GraphInputs,
    ) -> Result<(), String> {
        for entry in self.inputs.values_mut() {
            if let Some(stream) = &mut entry.stream {
                stream.bind_inputs(inputs)?;
            }
        }
        Ok(())
    }
    pub(super) fn initialize_controls(&self, document: &mixrelay_audio_graph::GraphDocument) {
        // Vacant staged slots borrow live selection leases. Their controls belong
        // to the executing plan until publication and activation succeed.
        for owner in self.inputs.values().chain(self.outputs.values()) {
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

    pub(super) fn observe_controls(&self, controls: &mixrelay_render::RuntimeControlBank) {
        for owner in self.inputs.values().chain(self.outputs.values()) {
            let enabled = controls
                .snapshot(&owner.selected.node_id)
                .is_some_and(|(control, _)| control.enabled);
            owner.selected.enabled.store(enabled, Ordering::Release);
        }
    }
    #[cfg(test)]
    #[cfg(feature = "native-lab")]
    pub(super) fn input_owner(&self, id: &str) -> Option<usize> {
        self.inputs
            .get(id)?
            .stream
            .as_ref()
            .map(NativeStream::allocation_id)
    }

    pub(super) fn discard_inputs(&mut self) {
        for entry in self.inputs.values_mut() {
            if let Some(stream) = &mut entry.stream {
                stream.discard_input();
            }
        }
    }
    pub(super) fn finish_input_block(&mut self) {
        for entry in self.inputs.values_mut() {
            if let Some(stream) = &mut entry.stream {
                stream.finish_capture_block();
            }
        }
    }
    pub(super) fn shares_clock(&self, id: u32, rate: u32) -> bool {
        self.inputs
            .values()
            .chain(self.outputs.values())
            .all(|entry| {
                entry
                    .stream
                    .as_ref()
                    .is_some_and(|stream| stream.shares_clock(id, rate))
            })
    }
}
impl ObservedStream {
    fn matches_recovery(&self, next: &Self) -> bool {
        Arc::ptr_eq(&self.connected, &next.connected) && Arc::ptr_eq(&self.selected, &next.selected)
    }

    fn accepts_replacement(&self, next: &Self) -> bool {
        self.selected.enabled.load(Ordering::Acquire)
            && (self
                .stream
                .as_ref()
                .is_some_and(|stream| !stream.connected())
                || next.replaces.is_some_and(|expected| {
                    expected == self.selected.serial.load(Ordering::Acquire)
                        && next.serial != expected
                }))
    }
    fn new(id: &str, stream: NativeStream, serial: u64) -> Self {
        Self {
            stream: Some(stream),
            connected: Arc::new(AtomicBool::new(true)),
            selected: Arc::new(DeviceSelection::new(id, serial)),
            serial,
            replaces: None,
        }
    }
    fn ready(&self) -> bool {
        self.stream
            .as_ref()
            .is_some_and(|stream| stream.connected() && stream.ready())
    }
}

/// Read-only health shared with the recovery worker, never a shared stream/ring.
#[derive(Clone)]
pub(super) struct DeviceWatch {
    owners: super::native::StreamOwners,
    pub(super) generation: u64,
    spec: DeviceRequirements,
    inputs: BTreeMap<String, Arc<AtomicBool>>,
    outputs: BTreeMap<String, Arc<AtomicBool>>,
    input_selections: BTreeMap<String, Arc<DeviceSelection>>,
    output_selections: BTreeMap<String, Arc<DeviceSelection>>,
    pub(super) format: mixrelay_audio_graph::RenderFormat,
    #[cfg(test)]
    automatic_outputs: bool,
}
impl DeviceWatch {
    fn recovery_requests(
        &self,
        targets: &[mixrelay_linux_audio_io::PipeWireTarget],
    ) -> DeviceRequirements {
        let mut spec = self.disconnected();
        for (id, config) in &self.spec.inputs {
            if !matches!(
                config.selector,
                mixrelay_audio_io::DeviceSelector::DefaultInput
            ) || !self
                .input_selections
                .get(id)
                .is_some_and(|value| value.enabled.load(Ordering::Acquire))
            {
                continue;
            }
            let resolved = mixrelay_linux_audio_io::resolve_device_target(
                targets,
                &config.selector,
                DeviceDirection::Input,
                config.output_channels,
            )
            .ok()
            .and_then(|target| target.serial.parse::<u64>().ok());
            if self.changed_default(id, &config.selector, DeviceDirection::Input, resolved)
                && !spec.inputs.iter().any(|(pending, _)| pending == id)
            {
                spec.inputs.push((id.clone(), config.clone()));
            }
        }
        for (id, config) in &self.spec.outputs {
            if !matches!(
                config.selector,
                mixrelay_audio_io::DeviceSelector::DefaultOutput
            ) || !self
                .output_selections
                .get(id)
                .is_some_and(|value| value.enabled.load(Ordering::Acquire))
            {
                continue;
            }
            let resolved = mixrelay_linux_audio_io::resolve_device_target(
                targets,
                &config.selector,
                DeviceDirection::Output,
                config.input_channels,
            )
            .ok()
            .and_then(|target| target.serial.parse::<u64>().ok());
            if self.changed_default(id, &config.selector, DeviceDirection::Output, resolved)
                && !spec.outputs.iter().any(|(pending, _)| pending == id)
            {
                spec.outputs.push((id.clone(), config.clone()));
            }
        }
        spec
    }

    pub(super) fn needs_observation(&self) -> bool {
        !self.disconnected().is_empty()
            || self.spec.inputs.iter().any(|(id, config)| {
                self.input_selections
                    .get(id)
                    .is_some_and(|value| value.enabled.load(Ordering::Acquire))
                    && matches!(
                        config.selector,
                        mixrelay_audio_io::DeviceSelector::DefaultInput
                    )
            })
            || self.spec.outputs.iter().any(|(id, config)| {
                self.output_selections
                    .get(id)
                    .is_some_and(|value| value.enabled.load(Ordering::Acquire))
                    && matches!(
                        config.selector,
                        mixrelay_audio_io::DeviceSelector::DefaultOutput
                    )
            })
    }

    fn changed_default(
        &self,
        id: &str,
        selector: &mixrelay_audio_io::DeviceSelector,
        direction: DeviceDirection,
        resolved: Option<u64>,
    ) -> bool {
        let selections = match (selector, direction) {
            (mixrelay_audio_io::DeviceSelector::DefaultInput, DeviceDirection::Input) => {
                &self.input_selections
            }
            (mixrelay_audio_io::DeviceSelector::DefaultOutput, DeviceDirection::Output) => {
                &self.output_selections
            }
            _ => return false,
        };
        resolved.is_some_and(|serial| {
            selections.get(id).is_some_and(|current| {
                current.enabled.load(Ordering::Acquire)
                    && current.serial.load(Ordering::Acquire) != serial
            })
        })
    }

    pub(super) fn disconnected(&self) -> DeviceRequirements {
        DeviceRequirements {
            inputs: self
                .spec
                .inputs
                .iter()
                .filter(|(id, _)| {
                    self.input_selections
                        .get(id)
                        .is_none_or(|value| value.enabled.load(Ordering::Acquire))
                        && self
                            .inputs
                            .get(id)
                            .is_some_and(|value| !value.load(Ordering::Acquire))
                })
                .cloned()
                .collect(),
            outputs: self
                .spec
                .outputs
                .iter()
                .filter(|(id, _)| {
                    self.output_selections
                        .get(id)
                        .is_none_or(|value| value.enabled.load(Ordering::Acquire))
                        && self
                            .outputs
                            .get(id)
                            .is_some_and(|value| !value.load(Ordering::Acquire))
                })
                .cloned()
                .collect(),
        }
    }
}
impl DeviceStreams {
    #[cfg(test)]
    pub(super) fn open(spec: &DeviceRequirements, generation: u64) -> Result<Self, String> {
        Self::open_format(
            &super::native::StreamOwners::default(),
            spec,
            generation,
            super::render_format(),
            None,
        )
    }
    pub(super) fn open_format(
        owners: &super::native::StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        format: mixrelay_audio_graph::RenderFormat,
        previous: Option<&DeviceWatch>,
    ) -> Result<Self, String> {
        Self::open_with_policy(
            owners,
            spec,
            generation,
            format,
            previous,
            OpenPolicy::Initial,
        )
    }

    #[cfg(test)]
    /// Prepares the same functional owners for a polling publisher, not an RT benchmark.
    pub(super) fn open_standalone_format(
        owners: &super::native::StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        format: mixrelay_audio_graph::RenderFormat,
        previous: Option<&DeviceWatch>,
    ) -> Result<Self, String> {
        Self::open_with_policy(
            owners,
            spec,
            generation,
            format,
            previous,
            OpenPolicy::StandalonePublisher,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "native scope, revision and format accompany this prepared device request"
    )]
    fn open_with_policy(
        owners: &super::native::StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        format: mixrelay_audio_graph::RenderFormat,
        previous: Option<&DeviceWatch>,
        policy: OpenPolicy<'_>,
    ) -> Result<Self, String> {
        let (mut staged, required) = Self::stage(spec, generation, format, previous);
        staged.owners = owners.clone();
        #[cfg(test)]
        {
            staged.automatic_outputs = policy.automatic_outputs();
        }
        if required.is_empty() {
            return Ok(staged);
        }
        let targets = mixrelay_linux_audio_io::targets().map_err(|error| error.to_string())?;
        let mut opened = Self::open_inventory(
            owners,
            &required,
            staged.generation,
            &targets,
            policy,
            format,
        )?;
        staged.inputs.append(&mut opened.inputs);
        staged.outputs.append(&mut opened.outputs);
        Ok(staged)
    }

    pub(super) fn recover(watch: &DeviceWatch, stopping: &AtomicBool) -> Result<Self, String> {
        let policy = OpenPolicy::Recovery(stopping);
        #[cfg(test)]
        let policy = if watch.automatic_outputs {
            OpenPolicy::StandaloneRecovery(stopping)
        } else {
            policy
        };
        policy.check_cancelled()?;
        if !watch.needs_observation() {
            return Ok(Self {
                generation: watch.generation,
                ..Self::default()
            });
        }
        let targets = mixrelay_linux_audio_io::targets().map_err(|error| error.to_string())?;
        let spec = watch.recovery_requests(&targets);
        // Devices can return independently. One absent interface must not delay
        // another interface's recovery. Unresolvable requests remain watched.
        let available = DeviceRequirements {
            inputs: spec
                .inputs
                .iter()
                .take_while(|_| !stopping.load(Ordering::Acquire))
                .filter(|(_, config)| {
                    mixrelay_linux_audio_io::resolve_device_target(
                        &targets,
                        &config.selector,
                        DeviceDirection::Input,
                        config.output_channels,
                    )
                    .is_ok()
                })
                .cloned()
                .collect(),
            outputs: spec
                .outputs
                .iter()
                .take_while(|_| !stopping.load(Ordering::Acquire))
                .filter(|(_, config)| {
                    mixrelay_linux_audio_io::resolve_device_target(
                        &targets,
                        &config.selector,
                        DeviceDirection::Output,
                        config.input_channels,
                    )
                    .is_ok()
                })
                .cloned()
                .collect(),
        };
        let mut prepared = Self::open_inventory(
            &watch.owners,
            &available,
            watch.generation,
            &targets,
            policy,
            watch.format,
        )?;
        prepared.inherit_leases(watch);
        prepared.follow_defaults(watch);
        Ok(prepared)
    }

    fn follow_defaults(&mut self, watch: &DeviceWatch) {
        for (streams, selections) in [
            (&mut self.inputs, &watch.input_selections),
            (&mut self.outputs, &watch.output_selections),
        ] {
            for (id, stream) in streams {
                let follows_default = watch.spec.inputs.iter().any(|(binding, config)| {
                    binding == id
                        && matches!(
                            config.selector,
                            mixrelay_audio_io::DeviceSelector::DefaultInput
                        )
                }) || watch.spec.outputs.iter().any(|(binding, config)| {
                    binding == id
                        && matches!(
                            config.selector,
                            mixrelay_audio_io::DeviceSelector::DefaultOutput
                        )
                });
                if follows_default && let Some(selected) = selections.get(id) {
                    stream.replaces = Some(selected.serial.load(Ordering::Acquire));
                }
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "one preflight and acquisition transaction retains every partial native owner"
    )]
    fn open_inventory(
        owners: &super::native::StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        targets: &[mixrelay_linux_audio_io::PipeWireTarget],
        policy: OpenPolicy<'_>,
        format: mixrelay_audio_graph::RenderFormat,
    ) -> Result<Self, String> {
        policy.check_cancelled()?;
        // Validate the entire request before opening any native stream. The
        // registry serial pins this apply to the selected device, not a recycled ID.
        let inputs = spec
            .inputs
            .iter()
            .map(|(id, config)| {
                policy.check_cancelled()?;
                mixrelay_linux_audio_io::resolve_device_target(
                    targets,
                    &config.selector,
                    DeviceDirection::Input,
                    config.output_channels,
                )
                .map(|target| (id, config.output_channels, target))
                .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = spec
            .outputs
            .iter()
            .map(|(id, config)| {
                policy.check_cancelled()?;
                mixrelay_linux_audio_io::resolve_device_target(
                    targets,
                    &config.selector,
                    DeviceDirection::Output,
                    config.input_channels,
                )
                .map(|target| (id, config.input_channels, target))
                .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut streams = Self {
            owners: owners.clone(),
            generation,
            format: Some(format),
            #[cfg(test)]
            automatic_outputs: policy.automatic_outputs(),
            ..Self::default()
        };
        for (id, channels, target) in inputs {
            policy.check_cancelled()?;
            let serial = target_serial(target)?;
            let opened = NativeStream::open_format(
                owners,
                &format!("{}.device-input", super::sanitize_node_name(id)),
                &target.display_name,
                StreamRole::DeviceInput(&target.serial),
                channels,
                super::native_format(format),
            );
            match opened {
                Ok(stream) => {
                    streams
                        .inputs
                        .insert(id.clone(), ObservedStream::new(id, stream, serial));
                }
                Err(error) if !policy.allows_partial() => return Err(error),
                Err(_) => {}
            }
        }
        for (id, channels, target) in outputs {
            policy.check_cancelled()?;
            let serial = target_serial(target)?;
            let opened = NativeStream::open_format(
                owners,
                &format!("{}.device-output", super::sanitize_node_name(id)),
                &target.display_name,
                policy.output_role(&target.serial),
                channels,
                super::native_format(format),
            );
            match opened {
                Ok(stream) => {
                    streams
                        .outputs
                        .insert(id.clone(), ObservedStream::new(id, stream, serial));
                }
                Err(error) if !policy.allows_partial() => return Err(error),
                Err(_) => {}
            }
        }
        streams.wait_connected(policy)?;
        Ok(streams)
    }

    // PAUSED only means registered, not linked. Bound the control-plane
    // handshake and drop every partial owner on failure; the DSP never waits.
    fn wait_connected(&mut self, policy: OpenPolicy<'_>) -> Result<(), String> {
        let started = Instant::now();
        while !self.all_connected() {
            policy.check_cancelled()?;
            if started.elapsed() >= Duration::from_secs(2) {
                if policy.allows_partial() {
                    self.inputs.retain(|_, value| value.ready());
                    self.outputs.retain(|_, value| value.ready());
                    break;
                }
                return Err("PipeWire could not connect the selected audio device".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.outputs.is_empty()
    }

    fn all_connected(&self) -> bool {
        self.inputs
            .values()
            .chain(self.outputs.values())
            .all(ObservedStream::ready)
    }

    pub(super) fn watch(&self, spec: DeviceRequirements) -> DeviceWatch {
        DeviceWatch {
            owners: self.owners.clone(),
            #[cfg(test)]
            automatic_outputs: self.automatic_outputs,
            format: self.format.unwrap_or_else(super::render_format),
            generation: self.generation,
            spec,
            inputs: self
                .inputs
                .iter()
                .map(|(id, value)| (id.clone(), value.connected.clone()))
                .collect(),
            outputs: self
                .outputs
                .iter()
                .map(|(id, value)| (id.clone(), value.connected.clone()))
                .collect(),
            input_selections: self
                .inputs
                .iter()
                .map(|(id, value)| (id.clone(), value.selected.clone()))
                .collect(),
            output_selections: self
                .outputs
                .iter()
                .map(|(id, value)| (id.clone(), value.selected.clone()))
                .collect(),
        }
    }

    /// RT-only swaps existing slots. Keep original health cells so the control
    /// watcher follows the replacement. Healthy owners change only for a prepared
    /// default selection whose expected serial still matches the active owner.
    pub(super) fn replace_disconnected(&mut self, next: &mut Self) {
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

    pub(super) fn read(&mut self, node: &str, samples: &mut [f32]) -> TransferOutcome {
        samples.fill(0.0);
        self.inputs
            .get_mut(node)
            .and_then(|entry| entry.stream.as_mut())
            .map_or(TransferOutcome::complete(0), |stream| {
                if !stream.connected() {
                    return TransferOutcome {
                        frames: 0,
                        discontinuity: true,
                        ..TransferOutcome::complete(0)
                    };
                }
                stream.read_capture(samples)
            })
    }

    pub(super) fn write(&mut self, node: &str, samples: &[f32]) -> TransferOutcome {
        self.outputs
            .get_mut(node)
            .and_then(|entry| entry.stream.as_mut())
            .map_or(TransferOutcome::complete(0), |stream| {
                if !stream.connected() {
                    return TransferOutcome {
                        frames: 0,
                        discontinuity: true,
                        ..TransferOutcome::complete(0)
                    };
                }
                TransferOutcome::complete(stream.write(samples))
            })
    }

    /// Releases canonical physical output owners once per native cycle, including
    /// OFF nodes whose live transport must still drain without bypassing DSP gates.
    pub(super) fn trigger_outputs(&mut self, failures: &crate::processing_failure::FailureMailbox) {
        for stream in self
            .outputs
            .values_mut()
            .filter_map(|entry| entry.stream.as_mut())
        {
            if let mixrelay_linux_pipewire::OutputRelease::Rejected(code) = stream.trigger_output()
            {
                failures.publish_native(
                    crate::processing_failure::Failure::OutputReleaseRejected,
                    code,
                );
            }
        }
    }

    pub(super) fn take_report(&mut self) -> ProcessReport {
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

#[cfg(test)]
impl DeviceStreams {
    /// Observes acquired physical input/output owners without changing report baselines.
    #[cfg(feature = "native-lab")]
    pub(super) fn transfer_snapshot(&self) -> super::native::TransferSnapshot {
        let mut snapshot = super::native::TransferSnapshot::default();
        for stream in self
            .inputs
            .values()
            .chain(self.outputs.values())
            .filter_map(|entry| entry.stream.as_ref())
        {
            snapshot.include(stream);
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_serial_accepts_u64_identity_and_rejects_invalid_or_overflowing_text() {
        for (serial, expected) in [
            ("123", Ok(123)),
            ("18446744073709551615", Ok(u64::MAX)),
            ("not-a-serial", Err("invalid PipeWire serial")),
            ("18446744073709551616", Err("invalid PipeWire serial")),
        ] {
            let target = mixrelay_linux_audio_io::PipeWireTarget {
                object_id: 1,
                serial: serial.into(),
                node_name: None,
                process_id: None,
                application_id: None,
                executable: None,
                icon_name: None,
                display_name: String::new(),
                media_class: String::new(),
                channels: 2,
            };
            assert_eq!(target_serial(&target), expected.map_err(str::to_owned));
        }
    }

    #[test]
    fn only_recovery_allows_partial_open_and_carries_cancellation() {
        assert!(!OpenPolicy::Initial.allows_partial());
        assert!(OpenPolicy::Initial.check_cancelled().is_ok());
        assert!(!OpenPolicy::StandalonePublisher.allows_partial());
        assert!(OpenPolicy::StandalonePublisher.automatic_outputs());
        let stopping = AtomicBool::new(false);
        let recovery = OpenPolicy::Recovery(&stopping);
        let standalone_recovery = OpenPolicy::StandaloneRecovery(&stopping);
        assert_eq!(
            recovery.output_role("sink"),
            StreamRole::DeviceOutput("sink")
        );
        assert_eq!(
            standalone_recovery.output_role("sink"),
            StreamRole::AutomaticDeviceOutput("sink")
        );
        assert!(recovery.allows_partial());
        assert!(recovery.check_cancelled().is_ok());
        assert!(standalone_recovery.allows_partial());
        assert!(standalone_recovery.automatic_outputs());
        assert!(standalone_recovery.check_cancelled().is_ok());
        stopping.store(true, Ordering::Release);
        assert!(recovery.check_cancelled().is_err());
        assert!(standalone_recovery.check_cancelled().is_err());
    }

    #[test]
    fn cancelled_recovery_opens_no_native_resources() {
        let stopping = AtomicBool::new(true);
        assert!(
            DeviceStreams::open_inventory(
                &super::super::native::StreamOwners::default(),
                &DeviceRequirements::default(),
                3,
                &[],
                OpenPolicy::Recovery(&stopping),
                super::super::render_format()
            )
            .is_err()
        );
    }
    #[test]
    fn no_device_graph_does_not_need_a_pipewire_server() {
        let mut streams = DeviceStreams::open(&DeviceRequirements::default(), 1).unwrap();
        let mut input = [1.0; 4];
        assert_eq!(streams.read("missing", &mut input).frames, 0);
        assert_eq!(input.map(f32::to_bits), [0; 4]);
        assert_eq!(streams.write("missing", &input).frames, 0);
        assert!(
            !streams
                .watch(DeviceRequirements::default())
                .automatic_outputs
        );
        let standalone = DeviceStreams::open_standalone_format(
            &super::super::native::StreamOwners::default(),
            &DeviceRequirements::default(),
            2,
            super::super::render_format(),
            None,
        )
        .unwrap();
        assert!(
            standalone
                .watch(DeviceRequirements::default())
                .automatic_outputs
        );
    }
    #[test]
    fn gain_edits_do_not_reopen_native_devices() {
        let mut doc: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"id":"test","revision":1,"nodes":[
                {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":64,"gainDb":0.0}}
            ],"edges":[]
        })).unwrap();
        let before = DeviceRequirements::from_document(&doc);
        if let NodeConfig::AudioDeviceInput(config) = &mut doc.nodes[0].config {
            config.gain_db = -12.0;
        }
        assert!(before == DeviceRequirements::from_document(&doc));
        if let NodeConfig::AudioDeviceInput(config) = &mut doc.nodes[0].config {
            config.output_channels = 16;
        }
        assert!(before != DeviceRequirements::from_document(&doc));
    }
    #[test]
    fn health_watch_retries_only_disconnected_registered_streams() {
        let input = Arc::new(AtomicBool::new(true));
        let output = Arc::new(AtomicBool::new(true));
        let spec = DeviceRequirements {
            inputs: vec![(
                "mic".into(),
                AudioDeviceInputConfig {
                    selector: mixrelay_audio_io::DeviceSelector::DefaultInput,
                    output_channels: 64,
                    gain_db: 0.0,
                },
            )],
            outputs: vec![(
                "speaker".into(),
                AudioDeviceOutputConfig {
                    selector: mixrelay_audio_io::DeviceSelector::DefaultOutput,
                    input_channels: 16,
                },
            )],
        };
        let watch = DeviceWatch {
            owners: super::super::native::StreamOwners::default(),
            automatic_outputs: false,
            format: super::super::render_format(),
            generation: 3,
            spec,
            inputs: BTreeMap::from([("mic".into(), input.clone())]),
            outputs: BTreeMap::from([("speaker".into(), output.clone())]),
            input_selections: BTreeMap::new(),
            output_selections: BTreeMap::new(),
        };
        assert!(watch.disconnected().is_empty());
        input.store(false, Ordering::Release);
        assert_eq!(watch.disconnected().inputs.len(), 1);
        assert!(watch.disconnected().outputs.is_empty());
        input.store(true, Ordering::Release);
        output.store(false, Ordering::Release);
        assert!(watch.disconnected().inputs.is_empty());
        assert_eq!(watch.disconnected().outputs.len(), 1);
    }
}
