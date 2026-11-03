//! Native format owners are staged off RT, then revision-fenced with the DSP plan.
use super::{
    Arc, AtomicBool, CaptureWatch, DeviceStreams, DeviceWatch, Duration, EndpointControls,
    EndpointSet, EndpointSpecs, HostAudioBackendError, Instant, LinuxResourceSpec,
    LinuxResourceUpdate, LinuxVirtualEndpointBackend, Ordering, PipeWireCaptureSet, RenderFormat,
    thread,
};
use mixrelay_engine::PreparedGraphFormat;
#[cfg(test)]
#[path = "linux_format_tests.rs"]
mod tests;

pub(super) use mixrelay_audio_io::formats::{BUFFER_SIZES as BLOCKS, SAMPLE_RATES as RATES};
/// Callback-owned endpoint rate and quantum evidence for one native backend.
#[derive(Debug, Default)]
pub struct EndpointObservation {
    rate: std::sync::atomic::AtomicU32,
    // Internal callback diagnostics; this is not a user-selected fixed buffer size.
    quantum: std::sync::atomic::AtomicU64,
}
impl EndpointObservation {
    /// Returns a rate only when all active endpoint streams agree.
    #[must_use]
    pub fn rate(&self) -> Option<u32> {
        let rate = self.rate.load(Ordering::Acquire);
        (rate != 0).then_some(rate)
    }
    pub(super) fn observe(&self, endpoints: &EndpointSet) {
        let mut agreed = None;
        let mut quantum = 0;
        for stream in endpoints.streams() {
            let observed = stream.callback_quantum();
            if observed != 0 {
                quantum = observed;
            }
            let rate = (stream.negotiated_format() >> 32) as u32;
            if rate == 0 || agreed.is_some_and(|previous| previous != rate) {
                self.rate.store(0, Ordering::Release);
                self.quantum.store(0, Ordering::Release);
                return;
            }
            agreed = Some(rate);
        }
        // Only currently active native owners contribute; staged streams do not.
        self.rate.store(agreed.unwrap_or(0), Ordering::Release);
        self.quantum.store(quantum, Ordering::Release);
    }
}

pub struct Monitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Monitor {
    pub fn stop_observing(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    /// Polls the supplied Driver and domain callback off the audio thread.
    /// # Errors
    /// Returns the native thread creation failure without leaving a worker running.
    pub fn start(
        status: &Arc<super::AdapterRuntimeStatus>,
        driver: Arc<dyn super::EndpointDriver>,
        mut reconcile: impl FnMut() -> bool + Send + 'static,
    ) -> Result<Self, String> {
        let status = Arc::downgrade(status);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("mixrelay-pipewire-format-monitor".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    let Some(status) = status.upgrade() else {
                        break;
                    };
                    observe_driver(
                        &status,
                        driver.snapshot().ok().map(|snapshot| snapshot.state),
                    );
                    if !reconcile() {
                        break;
                    }
                    drop(status);
                    thread::park_timeout(Duration::from_millis(250));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

fn observe_driver(
    status: &super::AdapterRuntimeStatus,
    observed: Option<mixrelay_driver_control::DriverState>,
) {
    match observed {
        Some(state) => status.observe_driver_state(state),
        _ => status.forget_endpoint_observation(),
    }
}
impl Monitor {
    /// Stops and joins the monitor; a panic means the service did not stop cleanly.
    /// # Errors
    /// Returns an error if the native observation worker panicked.
    pub fn finish(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        self.stop_observing();
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "MixRelay endpoint format monitor panicked".to_owned())
        })
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_join() {
            tracing::error!(%error);
        }
    }
}

pub(super) const fn pack(format: RenderFormat) -> u64 {
    (format.sample_rate as u64) << 32 | format.block_frames as u64
}
pub(super) const fn unpack(value: u64) -> RenderFormat {
    let [h0, h1, h2, h3, l0, l1, l2, l3] = value.to_be_bytes();
    RenderFormat {
        sample_rate: u32::from_be_bytes([h0, h1, h2, h3]),
        block_frames: u32::from_be_bytes([l0, l1, l2, l3]),
    }
}

struct Reservation(Arc<LinuxVirtualEndpointBackend>);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.format_reserved.store(false, Ordering::Release);
    }
}
struct Prepared {
    reservation: Reservation,
    resources: Box<LinuxResourceUpdate>,
    spec: LinuxResourceSpec,
    format: RenderFormat,
    application_watch: CaptureWatch,
    device_watch: DeviceWatch,
    endpoint_controls: EndpointControls,
    playback: super::source_mute::OwnedPlan,
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
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active = spec;
        *backend
            .application_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(application_watch);
        *backend
            .device_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(device_watch);
        *backend
            .endpoint_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = endpoint_controls;
        backend.active_format.store(pack(format), Ordering::Release);
        backend.playback.stage(resources.revision, playback);
        backend
            .pending
            .store(Box::into_raw(resources), Ordering::Release);
        drop(active);
    }
}

pub(super) fn prepare(
    backend: &LinuxVirtualEndpointBackend,
    document: &mixrelay_audio_graph::GraphDocument,
    requested: RenderFormat,
    playback: super::source_mute::OwnedPlan,
) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
    let failure = |message: String| HostAudioBackendError::BackendFailure { message };
    mixrelay_audio_io::format_contract::validate_format(
        requested.sample_rate,
        requested.block_frames,
    )
    .map_err(|error| failure(error.to_string()))?;
    let owner = backend
        .self_ref
        .upgrade()
        .ok_or_else(|| failure("PipeWire backend stopped".into()))?;
    backend
        .format_reserved
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| failure("PipeWire format already reserved".into()))?;
    let reservation = Reservation(owner);
    if !backend.pending.load(Ordering::Acquire).is_null() {
        return Err(failure("PipeWire graph resources are still pending".into()));
    }
    let spec = LinuxResourceSpec::from_document(document)?;
    let endpoints = EndpointSet::prepare(
        &backend.stream_owners,
        &spec.endpoints,
        &EndpointSpecs::new(),
        requested,
        requested,
    )
    .map_err(failure)?;
    let endpoint_controls = endpoints
        .controls(&EndpointControls::default())
        .map_err(failure)?;
    let generation = backend
        .application_generation
        .fetch_add(1, Ordering::Relaxed)
        + 1;
    let (captures, application_watch) = PipeWireCaptureSet::open_format(
        &backend.stream_owners,
        &spec.captures,
        generation,
        requested,
        None,
    )
    .map_err(failure)?;
    let generation = backend.device_generation.fetch_add(1, Ordering::Relaxed) + 1;
    let devices = DeviceStreams::open_format(
        &backend.stream_owners,
        &spec.devices,
        generation,
        requested,
        None,
    )
    .map_err(failure)?;
    let device_watch = devices.watch(spec.devices.clone());
    let started = Instant::now();
    while !endpoints.ready() || !captures.formats_ready() {
        if started.elapsed() > Duration::from_secs(3) {
            return Err(failure(
                "PipeWire did not negotiate the requested stream format".into(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let mut resources = Box::new(LinuxResourceUpdate {
        revision: document.revision,
        endpoints: Some(Box::new(endpoints)),
        captures: Some(Box::new(captures)),
        devices: Some(Box::new(devices)),
    });
    backend
        .bind_format_inputs(&mut resources)
        .map_err(failure)?;
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
