//! Opening physical device streams: one preflight resolves every request before any
//! native stream opens, and connection waits are bounded.
use super::{DeviceRequest, DeviceStreams, DeviceWatch, ObservedStream};
use crate::runtime::{native_format, sanitize_node_name};
use mixrelay_audio_graph::RenderFormat;
use mixrelay_engine_core::resource_requirements::DeviceRequirements;
use mixrelay_linux_audio_io::PipeWireTarget;
use mixrelay_linux_pipewire::{NativeStream, StreamOwners, StreamRole};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Startup is atomic; recovery may admit independently returning devices and
/// always carries cancellation. These are not independent boolean choices.
#[derive(Clone, Copy, Debug)]
pub(super) enum OpenPolicy<'a> {
    Initial,
    Recovery(&'a AtomicBool),
    #[cfg(test)]
    StandalonePublisher,
    #[cfg(test)]
    StandaloneRecovery(&'a AtomicBool),
}

impl OpenPolicy<'_> {
    pub(super) fn check_cancelled(self) -> Result<(), String> {
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

    pub(super) const fn allows_partial(self) -> bool {
        match self {
            Self::Recovery(_) => true,
            #[cfg(test)]
            Self::StandaloneRecovery(_) => true,
            _ => false,
        }
    }

    pub(super) fn output_role(self, target: &str) -> StreamRole<'_> {
        match self {
            Self::Initial | Self::Recovery(_) => StreamRole::DeviceOutput(target),
            #[cfg(test)]
            Self::StandalonePublisher | Self::StandaloneRecovery(_) => {
                StreamRole::AutomaticDeviceOutput(target)
            }
        }
    }

    #[cfg(test)]
    pub(super) const fn automatic_outputs(self) -> bool {
        matches!(
            self,
            Self::StandalonePublisher | Self::StandaloneRecovery(_)
        )
    }
}

pub(super) fn target_serial(target: &PipeWireTarget) -> Result<u64, String> {
    target
        .serial
        .parse()
        .map_err(|_| "invalid PipeWire serial".to_owned())
}

/// A request resolved to its current registry target during preflight.
struct Resolved<'a> {
    id: &'a str,
    channels: u16,
    target: &'a PipeWireTarget,
}

fn resolve_all<'a, R: DeviceRequest>(
    requests: &'a [(String, R)],
    targets: &'a [PipeWireTarget],
    policy: OpenPolicy<'_>,
) -> Result<Vec<Resolved<'a>>, String> {
    requests
        .iter()
        .map(|(id, request)| {
            policy.check_cancelled()?;
            request
                .resolve(targets)
                .map(|target| Resolved {
                    id,
                    channels: request.channels(),
                    target,
                })
                .map_err(|error| error.to_string())
        })
        .collect()
}

/// Recovery opens only the requests whose device is present again; the rest stay
/// watched. Stops admitting requests once cancellation is observed.
fn available<R: DeviceRequest>(
    requests: &[(String, R)],
    targets: &[PipeWireTarget],
    stopping: &AtomicBool,
) -> Vec<(String, R)> {
    requests
        .iter()
        .take_while(|_| !stopping.load(Ordering::Acquire))
        .filter(|(_, request)| request.resolve(targets).is_ok())
        .cloned()
        .collect()
}

impl DeviceStreams {
    #[cfg(test)]
    pub(crate) fn open(spec: &DeviceRequirements, generation: u64) -> Result<Self, String> {
        Self::open_format(
            &StreamOwners::default(),
            spec,
            generation,
            crate::runtime::render_format(),
            None,
        )
    }

    pub(crate) fn open_format(
        owners: &StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        format: RenderFormat,
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

    /// Prepares the same functional owners for a polling publisher, not an RT benchmark.
    #[cfg(test)]
    pub(crate) fn open_standalone_format(
        owners: &StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        format: RenderFormat,
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
        owners: &StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        format: RenderFormat,
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

    /// Reopens disconnected bindings and bindings whose system default moved. Devices
    /// return independently, so one absent interface never delays another's recovery.
    pub(crate) fn recover(watch: &DeviceWatch, stopping: &AtomicBool) -> Result<Self, String> {
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
        let available = DeviceRequirements {
            inputs: available(&spec.inputs, &targets, stopping),
            outputs: available(&spec.outputs, &targets, stopping),
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

    /// Marks recovered default bindings with the serial they replace, so the audio
    /// thread can swap out a healthy owner only if that selection is still active.
    fn follow_defaults(&mut self, watch: &DeviceWatch) {
        let follows_default = |id: &str| {
            watch
                .spec
                .inputs
                .iter()
                .any(|(binding, request)| binding == id && request.follows_default())
                || watch
                    .spec
                    .outputs
                    .iter()
                    .any(|(binding, request)| binding == id && request.follows_default())
        };
        for (streams, selections) in [
            (&mut self.inputs, &watch.input_selections),
            (&mut self.outputs, &watch.output_selections),
        ] {
            for (id, stream) in streams {
                if follows_default(id)
                    && let Some(selected) = selections.get(id)
                {
                    stream.replaces = Some(selected.serial.load(Ordering::Acquire));
                }
            }
        }
    }

    /// Validates the entire request before opening any native stream. The registry
    /// serial pins this apply to the selected device, not a recycled ID.
    #[expect(
        clippy::too_many_arguments,
        reason = "one preflight and acquisition transaction retains every partial native owner"
    )]
    pub(super) fn open_inventory(
        owners: &StreamOwners,
        spec: &DeviceRequirements,
        generation: u64,
        targets: &[PipeWireTarget],
        policy: OpenPolicy<'_>,
        format: RenderFormat,
    ) -> Result<Self, String> {
        policy.check_cancelled()?;
        let inputs = resolve_all(&spec.inputs, targets, policy)?;
        let outputs = resolve_all(&spec.outputs, targets, policy)?;
        let mut streams = Self {
            owners: owners.clone(),
            generation,
            format: Some(format),
            #[cfg(test)]
            automatic_outputs: policy.automatic_outputs(),
            ..Self::default()
        };
        open_each(&mut streams.inputs, inputs, policy, |device| {
            NativeStream::open_format(
                owners,
                &format!("{}.device-input", sanitize_node_name(device.id)),
                &device.target.display_name,
                StreamRole::DeviceInput(&device.target.serial),
                device.channels,
                native_format(format),
            )
        })?;
        open_each(&mut streams.outputs, outputs, policy, |device| {
            NativeStream::open_format(
                owners,
                &format!("{}.device-output", sanitize_node_name(device.id)),
                &device.target.display_name,
                policy.output_role(&device.target.serial),
                device.channels,
                native_format(format),
            )
        })?;
        streams.wait_connected(policy)?;
        Ok(streams)
    }

    // PAUSED only means registered, not linked. Bound the control-plane
    // handshake and drop every partial owner on failure; the DSP never waits.
    fn wait_connected(&mut self, policy: OpenPolicy<'_>) -> Result<(), String> {
        let started = Instant::now();
        while !self.slots().all(ObservedStream::ready) {
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
}

/// Opens each resolved device. Startup fails on the first error; recovery skips a
/// device that fails to open so the others can still return.
fn open_each(
    slots: &mut BTreeMap<String, ObservedStream>,
    devices: Vec<Resolved<'_>>,
    policy: OpenPolicy<'_>,
    mut open: impl FnMut(&Resolved<'_>) -> Result<NativeStream, String>,
) -> Result<(), String> {
    for device in devices {
        policy.check_cancelled()?;
        let serial = target_serial(device.target)?;
        match open(&device) {
            Ok(stream) => {
                slots.insert(
                    device.id.to_owned(),
                    ObservedStream::new(device.id, stream, serial),
                );
            }
            Err(error) if !policy.allows_partial() => return Err(error),
            Err(_) => {}
        }
    }
    Ok(())
}
