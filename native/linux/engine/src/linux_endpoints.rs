use super::{
    HostAudioBackendError, NativeStream, RenderFormat, VirtualEndpointSpec, native::StreamRole,
};
use mixrelay_audio_io::process::{InputActivity, ProcessReport, TransferOutcome};
pub(super) use mixrelay_linux_driver::native_name;
#[cfg(test)]
pub(super) use mixrelay_linux_driver::native_names;
use std::collections::BTreeMap;

#[path = "linux_endpoint_properties.rs"]
mod properties;
pub(super) use properties::{EndpointControls, EndpointEdit};

/// One composed binding per canonical device; graph references never own native streams.
pub(super) type EndpointSpecs = BTreeMap<String, VirtualEndpointSpec>;

#[derive(Default)]
pub(super) struct EndpointSet {
    nodes: BTreeMap<String, EndpointSlot>,
}

struct EndpointSlot {
    spec: VirtualEndpointSpec,
    format: RenderFormat,
    // Vacant only in a prepared/retired transfer envelope, never an active owner.
    owner: Option<Endpoint>,
}

struct Endpoint {
    input: Option<NativeStream>,
    output: Option<NativeStream>,
    silence: Box<[f32]>,
    output_channels: usize,
    input_read: bool,
    output_written: bool,
}

// Display metadata is not a native PCM acquisition boundary.
fn same_transport(left: &VirtualEndpointSpec, right: &VirtualEndpointSpec) -> bool {
    left.id == right.id
        && left.render == right.render
        && left.capture == right.capture
        && left.default_sample_rate == right.default_sample_rate
        && left.supported_sample_rates == right.supported_sample_rates
}

pub(super) fn specs(
    bindings: Vec<(String, VirtualEndpointSpec)>,
) -> Result<EndpointSpecs, HostAudioBackendError> {
    let mut result = BTreeMap::new();
    let mut identities = std::collections::BTreeSet::new();
    for (id, spec) in bindings {
        if !identities.insert(spec.id.clone()) || result.insert(id, spec).is_some() {
            return Err(HostAudioBackendError::BackendFailure {
                message: "duplicate canonical virtual device binding".into(),
            });
        }
    }
    Ok(result)
}

impl Endpoint {
    fn open(
        owners: &super::native::StreamOwners,
        spec: &VirtualEndpointSpec,
        format: RenderFormat,
    ) -> Result<Self, String> {
        let name = native_name(&spec.id);
        // Graph reads the Driver-owned mixed capture, never OS playback directly.
        let input = spec
            .capture
            .map(|stream| {
                NativeStream::open_format(
                    owners,
                    &format!("{name}.graph-input"),
                    &spec.display_name,
                    StreamRole::DeviceInput(&format!("{name}.capture")),
                    stream.channels,
                    super::native_format(format),
                )
            })
            .transpose()?;
        // Only graph contributions enter the existing Driver render sink.
        let output = spec
            .render
            .map(|stream| {
                NativeStream::open_format(
                    owners,
                    &format!("{name}.graph-output"),
                    &spec.display_name,
                    StreamRole::GraphContribution(&format!("{name}.render")),
                    stream.channels,
                    super::native_format(format),
                )
            })
            .transpose()?;
        let output_channels = spec.render.map_or(0, |stream| usize::from(stream.channels));
        let frames = usize::try_from(format.block_frames)
            .map_err(|_| "graph contribution block does not fit this platform")?;
        let samples = frames
            .checked_mul(output_channels)
            .ok_or("graph contribution block size overflow")?;
        Ok(Self {
            input,
            output,
            silence: mixrelay_audio_io::pcm_buffer::zeroed(samples)
                .map_err(|error| error.to_string())?
                .into_boxed_slice(),
            output_channels,
            input_read: false,
            output_written: false,
        })
    }

    // Native devices outlive individual graph routes. Keep their transport fresh
    // when no binding runs, without retaining DSP or replaying queued OFF audio.
    fn finish_block(
        &mut self,
        frames: u32,
    ) -> Result<ProcessReport, crate::processing_failure::Failure> {
        if !std::mem::take(&mut self.input_read)
            && let Some(render) = &mut self.input
        {
            render.discard_input();
        }
        let mut report = ProcessReport::default();
        if !std::mem::take(&mut self.output_written)
            && let Some(capture) = &mut self.output
        {
            let samples = usize::try_from(frames)
                .ok()
                .and_then(|frames| frames.checked_mul(self.output_channels))
                .ok_or(crate::processing_failure::Failure::EndpointSilenceCapacity)?;
            let silence = self
                .silence
                .get(..samples)
                .ok_or(crate::processing_failure::Failure::EndpointSilenceCapacity)?;
            report.output_overflows = u64::from(capture.write(silence) != frames);
        }
        Ok(report)
    }
}

impl EndpointSet {
    pub(super) fn bind_inputs(
        &mut self,
        inputs: &mixrelay_pipewire::GraphInputs,
    ) -> Result<(), String> {
        for slot in self.nodes.values_mut() {
            if let Some(owner) = &mut slot.owner
                && let Some(render) = &mut owner.input
            {
                render.bind_inputs(inputs)?;
            }
        }
        Ok(())
    }
    /// Prepares only changed owners off RT. The processor exchange validates and
    /// transfers every retained slot before reclaiming the displaced envelope.
    pub(super) fn prepare(
        owners: &super::native::StreamOwners,
        desired: &EndpointSpecs,
        previous: &EndpointSpecs,
        format: RenderFormat,
        previous_format: RenderFormat,
    ) -> Result<Self, String> {
        let mut nodes = BTreeMap::new();
        for (id, spec) in desired {
            spec.validate_contract(super::MAX_ENDPOINT_CHANNELS, super::MAX_ENDPOINT_CHANNELS)
                .map_err(|error| error.to_string())?;
            let retained = format == previous_format
                && previous
                    .get(id)
                    .is_some_and(|previous| same_transport(previous, spec));
            let owner = if retained {
                None
            } else {
                Some(Endpoint::open(owners, spec, format)?)
            };
            nodes.insert(
                id.clone(),
                EndpointSlot {
                    spec: spec.clone(),
                    format,
                    owner,
                },
            );
        }
        Ok(Self { nodes })
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(super) fn streams(&self) -> impl Iterator<Item = &NativeStream> {
        self.nodes
            .values()
            .filter_map(|slot| slot.owner.as_ref())
            .flat_map(|owner| owner.input.iter().chain(owner.output.iter()))
    }

    /// Prepared vacant slots are validated against their active owner separately.
    pub(super) fn ready(&self) -> bool {
        self.streams().all(NativeStream::ready)
    }

    pub(super) fn shares_clock(&self, id: u32, rate: u32) -> bool {
        self.streams().all(|stream| stream.shares_clock(id, rate))
    }

    pub(super) fn can_retain_from(&self, active: &Self) -> bool {
        self.nodes.iter().all(|(id, slot)| {
            slot.owner.is_some()
                || active.nodes.get(id).is_some_and(|current| {
                    current.owner.is_some()
                        && same_transport(&current.spec, &slot.spec)
                        && current.format == slot.format
                })
        })
    }

    /// RT-only, after transaction-wide preflight. No native calls except a bounded
    /// discard for freshly opened capture-input queues; retained PCM/cursors stay intact.
    pub(super) fn retain_from(&mut self, active: &mut Self) {
        for (id, slot) in &mut self.nodes {
            if let Some(owner) = &mut slot.owner {
                if let Some(render) = &mut owner.input {
                    render.discard_input();
                }
            } else if let Some(current) = active.nodes.get_mut(id) {
                std::mem::swap(&mut slot.owner, &mut current.owner);
            }
        }
    }

    pub(super) fn read(&mut self, id: &str, destination: &mut [f32]) -> TransferOutcome {
        let Some(owner) = self.nodes.get_mut(id).and_then(|slot| slot.owner.as_mut()) else {
            destination.fill(0.0);
            return TransferOutcome::complete(0);
        };
        owner.input_read = true;
        let Some(stream) = &mut owner.input else {
            destination.fill(0.0);
            return TransferOutcome::complete(0);
        };
        let activity = stream.input_activity();
        if activity != InputActivity::Active {
            destination.fill(0.0);
            stream.discard_input();
            return TransferOutcome {
                frames: 0,
                discontinuity: false,
                activity,
                source_sample_faults: 0,
            };
        }
        stream.read_capture(destination)
    }

    pub(super) fn write(&mut self, id: &str, source: &[f32]) -> TransferOutcome {
        let Some(owner) = self.nodes.get_mut(id).and_then(|slot| slot.owner.as_mut()) else {
            return TransferOutcome::complete(0);
        };
        owner.output_written = true;
        let frames = owner
            .output
            .as_mut()
            .map_or(0, |stream| stream.write(source));
        TransferOutcome::complete(frames)
    }

    /// Completes unused transport directions and resets cycle-local access flags.
    /// Rejects a block larger than the off-RT prepared silence capacity.
    pub(super) fn finish_block(
        &mut self,
        frames: u32,
    ) -> Result<ProcessReport, crate::processing_failure::Failure> {
        let mut report = ProcessReport::default();
        for owner in self
            .nodes
            .values_mut()
            .filter_map(|slot| slot.owner.as_mut())
        {
            report.output_overflows += owner.finish_block(frames)?.output_overflows;
        }
        Ok(report)
    }

    pub(super) fn take_report(&mut self) -> ProcessReport {
        let mut report = ProcessReport::default();
        for owner in self
            .nodes
            .values_mut()
            .filter_map(|slot| slot.owner.as_mut())
        {
            for stream in owner.input.iter_mut().chain(owner.output.iter_mut()) {
                let next = stream.take_report();
                report.input_starvations += next.input_starvations;
                report.output_overflows += next.output_overflows;
                report.discontinuities += next.discontinuities;
            }
        }
        report
    }

    /// Releases each Driver render contribution once after all native-cycle slices.
    /// Retired/vacant slots and capture inputs never receive output triggers.
    pub(super) fn trigger_outputs(&mut self, failures: &crate::processing_failure::FailureMailbox) {
        for stream in self
            .nodes
            .values_mut()
            .filter_map(|slot| slot.owner.as_mut())
            .filter_map(|owner| owner.output.as_mut())
        {
            if let mixrelay_pipewire::OutputRelease::Rejected(code) = stream.trigger_output() {
                failures.publish_native(
                    crate::processing_failure::Failure::OutputReleaseRejected,
                    code,
                );
            }
        }
    }
}

#[cfg(test)]
impl EndpointSet {
    /// Observes current canonical transports; graph references do not add owners.
    #[cfg(feature = "native-lab")]
    pub(super) fn transfer_snapshot(&self) -> super::native::TransferSnapshot {
        let mut snapshot = super::native::TransferSnapshot::default();
        for owner in self.nodes.values().filter_map(|slot| slot.owner.as_ref()) {
            for stream in owner.input.iter().chain(owner.output.iter()) {
                snapshot.include(stream);
            }
        }
        snapshot
    }
}

#[cfg(test)]
#[path = "linux_endpoint_tests.rs"]
mod tests;
