//! Linux graph resource backend: prepares native owners off RT and publishes them
//! atomically with the DSP plan revision.
use crate::{
    applications::{CaptureSet, CaptureWatch, identity},
    devices::{DeviceStreams, DeviceWatch},
    endpoints::{EndpointControls, EndpointEdit, EndpointSet, EndpointSpecs},
    format::{self, EndpointObservation},
    processing::failure::Failure,
    runtime::{EndpointDriver, MAX_ENDPOINT_CHANNELS},
    source_mute,
};
use mixrelay_audio_graph::{ApplicationCaptureRequest, GraphDocument, RenderFormat};
use mixrelay_audio_io::{
    HostAudioBackendError, VirtualEndpointCapabilities, recovery::RecoveryMailbox,
};
use mixrelay_engine_core::{
    GraphResourceBackend, PlaybackBindings, PreparedGraphFormat,
    resource_requirements::DeviceRequirements,
};
use mixrelay_linux_pipewire::{GraphInputs, StreamOwners};
use mixrelay_time::{MonotonicClock, Sleeper};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

mod driver_settings;
mod format_change;
mod publication;
mod recovery;
#[cfg(test)]
#[cfg(feature = "native-lab")]
mod resource_tests;
#[cfg(test)]
mod tests;

pub use recovery::{DeviceRecoveryWorker, start_recovery};

use publication::{ResourceSpec, ResourceUpdate, UpdateMailbox};

pub struct LinuxVirtualEndpointBackend {
    // First field: staged and retired native owners drop before any other state.
    updates: UpdateMailbox<ResourceUpdate>,
    stream_owners: StreamOwners,
    driver: Arc<dyn EndpointDriver>,
    observation: Arc<EndpointObservation>,
    self_ref: Weak<Self>,
    active_format: AtomicU64,
    format_reserved: AtomicBool,
    capabilities: VirtualEndpointCapabilities,
    active_spec: Mutex<ResourceSpec>,
    endpoint_controls: Mutex<EndpointControls>,
    device_generation: AtomicU64,
    device_watch: Mutex<Option<DeviceWatch>>,
    device_recovery: RecoveryMailbox<DeviceStreams>,
    application_generation: AtomicU64,
    application_watch: Mutex<Option<CaptureWatch>>,
    application_recovery: RecoveryMailbox<CaptureSet>,
    // Weak, control-only access. None is initial setup, never shutdown fallback.
    graph_inputs: Mutex<Option<GraphInputs>>,
    playback: source_mute::PlaybackController,
    #[cfg(test)]
    automatic_device_outputs: bool,
}

impl LinuxVirtualEndpointBackend {
    /// Prepares this backend's resource scope without opening native streams.
    /// `timer` bounds the off-RT `WirePlumber` source-mute protocol.
    #[must_use]
    pub fn new(
        driver: Arc<dyn EndpointDriver>,
        format: RenderFormat,
        observation: Arc<EndpointObservation>,
        timer: impl MonotonicClock + Sleeper + 'static,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self::prepare(weak, driver, format, observation, timer))
    }

    #[cfg(all(test, feature = "native-lab"))]
    fn new_standalone_device_publishers() -> Arc<Self> {
        Arc::new_cyclic(|self_ref| {
            let mut backend = Self::prepare(
                self_ref,
                crate::test_support::test_driver(),
                crate::runtime::render_format(),
                Arc::new(EndpointObservation::default()),
                mixrelay_time::StdClock::new(),
            );
            backend.automatic_device_outputs = true;
            backend
        })
    }

    fn prepare(
        self_ref: &Weak<Self>,
        driver: Arc<dyn EndpointDriver>,
        format: RenderFormat,
        observation: Arc<EndpointObservation>,
        timer: impl MonotonicClock + Sleeper + 'static,
    ) -> Self {
        Self {
            updates: UpdateMailbox::default(),
            stream_owners: StreamOwners::default(),
            driver,
            observation,
            self_ref: self_ref.clone(),
            active_format: AtomicU64::new(format::pack(format)),
            format_reserved: AtomicBool::new(false),
            capabilities: VirtualEndpointCapabilities {
                max_render_channels: MAX_ENDPOINT_CHANNELS,
                max_capture_channels: MAX_ENDPOINT_CHANNELS,
                supported_sample_rates: format::SAMPLE_RATES.to_vec(),
                supports_duplex: true,
                supports_dynamic_creation: true,
            },
            active_spec: Mutex::new(ResourceSpec::default()),
            endpoint_controls: Mutex::default(),
            device_generation: AtomicU64::new(0),
            device_watch: Mutex::new(None),
            device_recovery: RecoveryMailbox::default(),
            application_generation: AtomicU64::new(0),
            application_watch: Mutex::new(None),
            application_recovery: RecoveryMailbox::default(),
            graph_inputs: Mutex::new(None),
            playback: source_mute::PlaybackController::installed(timer),
            #[cfg(test)]
            automatic_device_outputs: false,
        }
    }

    /// The callback-owned endpoint observation the audio worker updates every cycle.
    pub(crate) fn observation(&self) -> &EndpointObservation {
        &self.observation
    }

    /// Recovery mailboxes whose single audio-thread consumer swaps replacement owners in.
    pub(crate) const fn recovery_mailboxes(
        &self,
    ) -> (
        &RecoveryMailbox<CaptureSet>,
        &RecoveryMailbox<DeviceStreams>,
    ) {
        (&self.application_recovery, &self.device_recovery)
    }

    /// Audio-thread activation of the update published for `revision`. Every retained
    /// slot is validated before any owner moves; a rejected update stays pending.
    pub(crate) fn activate_pending(
        &self,
        revision: u64,
        endpoints: &mut Box<EndpointSet>,
        captures: &mut Box<CaptureSet>,
        devices: &mut Box<DeviceStreams>,
    ) -> Result<(), Failure> {
        self.updates
            .activate(|update| update.transfer_into(revision, endpoints, captures, devices))
    }

    /// Drops resources the audio thread displaced. Called only off RT.
    pub(crate) fn reclaim_retired(&self) {
        self.updates.reclaim_retired();
    }

    /// Records the graph driver's inputs and binds any update staged before it started.
    pub(crate) fn register_inputs(
        &self,
        inputs: &GraphInputs,
    ) -> Result<(), mixrelay_linux_pipewire::DriverError> {
        let mut registered = self.graph_inputs.lock().map_err(|_| {
            mixrelay_linux_pipewire::DriverError::Input("driver registration poisoned".into())
        })?;
        // SAFETY: only the initial unpublished driver calls this hook. Its RT DSP
        // consumer has not started, and graph_inputs excludes control publication.
        unsafe {
            self.updates
                .edit_pending(|update| update.bind_inputs(inputs))
        }
        .transpose()
        .map_err(mixrelay_linux_pipewire::DriverError::Input)?;
        *registered = Some(inputs.clone());
        Ok(())
    }

    fn registered_inputs<R>(
        &self,
        bind: impl FnOnce(&GraphInputs) -> Result<R, String>,
    ) -> Result<R, String> {
        let registered = self
            .graph_inputs
            .lock()
            .map_err(|_| "driver registration poisoned")?;
        bind(registered.as_ref().ok_or("driver has not registered")?)
    }

    fn bind_application_inputs(&self, captures: &mut CaptureSet) -> Result<(), String> {
        self.registered_inputs(|inputs| {
            inputs.ensure_live().map_err(|error| error.to_string())?;
            captures.bind_inputs(inputs)
        })
    }

    fn bind_device_inputs(&self, devices: &mut DeviceStreams) -> Result<(), String> {
        self.registered_inputs(|inputs| {
            inputs.ensure_live().map_err(|error| error.to_string())?;
            devices.bind_inputs(inputs)
        })
    }

    fn bind_format_inputs(&self, update: &mut ResourceUpdate) -> Result<(), String> {
        self.registered_inputs(|inputs| update.bind_inputs(inputs))
    }

    fn prepare_capture_edit(
        &self,
        requests: &[(String, ApplicationCaptureRequest)],
        format: RenderFormat,
    ) -> Result<(Box<CaptureSet>, CaptureWatch), HostAudioBackendError> {
        let generation = self.application_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let previous = self
            .application_watch
            .lock()
            .map_err(|_| poisoned("application recovery state"))?
            .clone();
        CaptureSet::open_format(
            &self.stream_owners,
            requests,
            generation,
            format,
            previous.as_ref(),
        )
        .map(|(captures, watch)| (Box::new(captures), watch))
        .map_err(backend_failure)
    }

    fn prepare_device_edit(
        &self,
        spec: &DeviceRequirements,
        format: RenderFormat,
    ) -> Result<Box<DeviceStreams>, HostAudioBackendError> {
        let generation = self.device_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let previous = self
            .device_watch
            .lock()
            .map_err(|_| poisoned("device recovery state"))?
            .clone();
        #[cfg(test)]
        if self.automatic_device_outputs {
            return DeviceStreams::open_standalone_format(
                &self.stream_owners,
                spec,
                generation,
                format,
                previous.as_ref(),
            )
            .map(Box::new)
            .map_err(backend_failure);
        }
        DeviceStreams::open_format(
            &self.stream_owners,
            spec,
            generation,
            format,
            previous.as_ref(),
        )
        .map(Box::new)
        .map_err(backend_failure)
    }

    fn prepare_endpoint_edit(
        &self,
        next: &EndpointSpecs,
        previous: &EndpointSpecs,
        format: RenderFormat,
    ) -> Result<Option<EndpointEdit>, HostAudioBackendError> {
        if next == previous {
            return Ok(None);
        }
        let controls = self
            .endpoint_controls
            .lock()
            .map_err(|_| poisoned("PipeWire endpoint control state"))?;
        EndpointEdit::prepare(&self.stream_owners, next, previous, &controls, format).map(Some)
    }

    fn publish(&self, mut update: ResourceUpdate) -> Result<(), HostAudioBackendError> {
        let registered = self
            .graph_inputs
            .lock()
            .map_err(|_| backend_failure("driver registration poisoned".into()))?;
        if let Some(inputs) = registered.as_ref() {
            update.bind_inputs(inputs).map_err(backend_failure)?;
        }
        if self.format_reserved.load(Ordering::Acquire) {
            return Err(backend_failure("PipeWire format update reserved".into()));
        }
        self.updates.publish(update).map_err(|_rejected| {
            backend_failure("a PipeWire endpoint update is already pending".into())
        })
    }

    fn publish_edit(
        &self,
        mut update: ResourceUpdate,
        endpoints: Option<EndpointEdit>,
    ) -> Result<(), HostAudioBackendError> {
        let Some(endpoints) = endpoints else {
            return self.publish(update);
        };
        let mut controls = self
            .endpoint_controls
            .lock()
            .map_err(|_| poisoned("PipeWire endpoint control state"))?;
        *controls = endpoints.publish(|endpoints| {
            update.endpoints = Some(endpoints);
            self.publish(update)
        })?;
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "native-lab")]
impl LinuxVirtualEndpointBackend {
    pub(crate) fn application_watch(&self) -> Option<CaptureWatch> {
        self.application_watch
            .lock()
            .expect("live application watch")
            .clone()
    }
}

impl GraphResourceBackend for LinuxVirtualEndpointBackend {
    fn execution_released(&self) -> bool {
        self.stream_owners.active_owners() == 0
    }

    fn resolve_application_capture(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<mixrelay_audio_graph::composition::SharedSource, HostAudioBackendError> {
        identity::resolve(request)
    }

    fn device_inventory(
        &self,
        _selectors: &[&mixrelay_audio_io::DeviceSelector],
    ) -> Result<Vec<mixrelay_audio_io::AudioDeviceDescriptor>, HostAudioBackendError> {
        mixrelay_linux_audio_io::devices()
    }

    fn prepare_format_change(
        &self,
        document: &GraphDocument,
        playback: PlaybackBindings<'_>,
        _current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        let playback = self.playback.prepare_plan(&playback)?;
        format_change::prepare(self, document, requested, playback)
    }

    fn reconcile(
        &self,
        document: &GraphDocument,
        playback: PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        let playback = self.playback.prepare_plan(&playback)?;
        let next = ResourceSpec::from_document(document)?;
        let format = format::unpack(self.active_format.load(Ordering::Acquire));
        let mut active = self
            .active_spec
            .lock()
            .map_err(|_| poisoned("PipeWire endpoint state"))?;
        if *active == next {
            self.playback.stage(document.revision, playback);
            return Ok(());
        }
        let endpoints = self.prepare_endpoint_edit(&next.endpoints, &active.endpoints, format)?;
        let (captures, mut application_watch) = if active.captures == next.captures {
            (None, None)
        } else {
            let (captures, watch) = self.prepare_capture_edit(&next.captures, format)?;
            (Some(captures), Some(watch))
        };
        let devices = if active.devices == next.devices {
            None
        } else {
            Some(self.prepare_device_edit(&next.devices, format)?)
        };
        let watch = devices.as_ref().map(|streams| {
            streams.initialize_controls(document);
            streams.watch(next.devices.clone())
        });
        // Lock before publication so poisoning cannot leave a staged edit with
        // an unregistered watcher. Recovery never holds this lock during I/O.
        let mut registered = self
            .device_watch
            .lock()
            .map_err(|_| poisoned("device recovery state"))?;
        let mut registered_applications = self
            .application_watch
            .lock()
            .map_err(|_| poisoned("application recovery state"))?;
        if let (Some(prepared), Some(current)) =
            (application_watch.as_mut(), registered_applications.as_ref())
        {
            prepared.refresh_leases(current);
        }
        self.publish_edit(
            ResourceUpdate {
                revision: document.revision,
                endpoints: None,
                captures,
                devices,
            },
            endpoints,
        )?;
        if let Some(watch) = watch {
            *registered = Some(watch);
        }
        if let Some(watch) = application_watch {
            *registered_applications = Some(watch);
        }
        *active = next;
        self.playback.stage(document.revision, playback);
        Ok(())
    }
}

impl Drop for LinuxVirtualEndpointBackend {
    fn drop(&mut self) {
        self.playback.clear_all();
    }
}

fn backend_failure(message: String) -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure { message }
}

fn poisoned(state: &str) -> HostAudioBackendError {
    backend_failure(format!("{state} is poisoned"))
}
