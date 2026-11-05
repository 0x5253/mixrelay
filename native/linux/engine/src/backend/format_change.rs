//! Live render-format change: every native owner is opened off RT in the requested
//! format, then published as one update fenced by the DSP plan revision.
use super::{LinuxVirtualEndpointBackend, ResourceSpec, ResourceUpdate, backend_failure};
use crate::{
    applications::{CaptureSet, CaptureWatch},
    devices::{DeviceStreams, DeviceWatch},
    endpoints::{EndpointControls, EndpointSet, EndpointSpecs},
    format, source_mute,
};
use mixrelay_audio_graph::{GraphDocument, RenderFormat};
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::PreparedGraphFormat;
use std::{
    sync::{Arc, PoisonError, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

/// Holds the backend's exclusive format reservation; dropping it releases publication.
struct Reservation(Arc<LinuxVirtualEndpointBackend>);

impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.format_reserved.store(false, Ordering::Release);
    }
}

struct Prepared {
    reservation: Reservation,
    resources: Box<ResourceUpdate>,
    spec: ResourceSpec,
    format: RenderFormat,
    application_watch: CaptureWatch,
    device_watch: DeviceWatch,
    endpoint_controls: EndpointControls,
    playback: source_mute::OwnedPlan,
}

impl PreparedGraphFormat for Prepared {
    fn commit(self: Box<Self>) {
        let Self {
            reservation,
            resources,
            spec,
            format,
            application_watch,
            device_watch,
            endpoint_controls,
            playback,
        } = *self;
        let backend = &reservation.0;
        // Controller serialization plus reservation excludes another publication.
        // All owners and the envelope already exist; callback only swaps them.
        let mut active = backend
            .active_spec
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *active = spec;
        *backend
            .application_watch
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(application_watch);
        *backend
            .device_watch
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(device_watch);
        *backend
            .endpoint_controls
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = endpoint_controls;
        backend
            .active_format
            .store(format::pack(format), Ordering::Release);
        backend.playback.stage(resources.revision, playback);
        backend.updates.publish_reserved(resources);
        drop(active);
    }
}

pub(super) fn prepare(
    backend: &LinuxVirtualEndpointBackend,
    document: &GraphDocument,
    requested: RenderFormat,
    playback: source_mute::OwnedPlan,
) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
    mixrelay_audio_io::format_contract::validate_format(
        requested.sample_rate,
        requested.block_frames,
    )
    .map_err(|error| backend_failure(error.to_string()))?;
    let reservation = reserve(backend)?;
    if backend.updates.has_pending() {
        return Err(backend_failure(
            "PipeWire graph resources are still pending".into(),
        ));
    }
    let spec = ResourceSpec::from_document(document)?;
    let endpoints = EndpointSet::prepare(
        &backend.stream_owners,
        &spec.endpoints,
        &EndpointSpecs::new(),
        requested,
        requested,
    )
    .map_err(backend_failure)?;
    let endpoint_controls = endpoints
        .controls(&EndpointControls::default())
        .map_err(backend_failure)?;
    let generation = backend
        .application_generation
        .fetch_add(1, Ordering::Relaxed)
        + 1;
    let (captures, application_watch) = CaptureSet::open_format(
        &backend.stream_owners,
        &spec.captures,
        generation,
        requested,
        None,
    )
    .map_err(backend_failure)?;
    let generation = backend.device_generation.fetch_add(1, Ordering::Relaxed) + 1;
    let devices = DeviceStreams::open_format(
        &backend.stream_owners,
        &spec.devices,
        generation,
        requested,
        None,
    )
    .map_err(backend_failure)?;
    let device_watch = devices.watch(spec.devices.clone());
    wait_for_negotiation(&endpoints, &captures)?;
    let mut resources = Box::new(ResourceUpdate {
        revision: document.revision,
        endpoints: Some(Box::new(endpoints)),
        captures: Some(Box::new(captures)),
        devices: Some(Box::new(devices)),
    });
    backend
        .bind_format_inputs(&mut resources)
        .map_err(backend_failure)?;
    Ok(Box::new(Prepared {
        reservation,
        spec,
        format: requested,
        application_watch,
        device_watch,
        endpoint_controls,
        playback,
        resources,
    }))
}

fn reserve(backend: &LinuxVirtualEndpointBackend) -> Result<Reservation, HostAudioBackendError> {
    let owner = backend
        .self_ref
        .upgrade()
        .ok_or_else(|| backend_failure("PipeWire backend stopped".into()))?;
    backend
        .format_reserved
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| backend_failure("PipeWire format already reserved".into()))?;
    Ok(Reservation(owner))
}

fn wait_for_negotiation(
    endpoints: &EndpointSet,
    captures: &CaptureSet,
) -> Result<(), HostAudioBackendError> {
    let started = Instant::now();
    while !endpoints.ready() || !captures.formats_ready() {
        if started.elapsed() > Duration::from_secs(3) {
            return Err(backend_failure(
                "PipeWire did not negotiate the requested stream format".into(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "native-lab")]
mod tests;
