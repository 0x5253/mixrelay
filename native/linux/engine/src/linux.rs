//! Linux runtime assembly and atomic publication of native resources.
use mixrelay_audio_graph::{
    ApplicationCaptureRequest, CompileContext, CompileLimits, GraphCompiler, GraphDocument,
    RenderFormat,
};
use mixrelay_audio_io::session::{AudioBackendKind, CURRENT_TIMER_FORMAT};
use mixrelay_audio_io::{
    HostAudioBackendError, VirtualEndpointBackend, VirtualEndpointCapabilities, VirtualEndpointSpec,
};
#[cfg(test)]
use mixrelay_engine_core::default_peak_meter_bank;
#[cfg(all(test, feature = "native-lab"))]
use mixrelay_engine_core::{EngineGraphController, EngineGraphControllerConfig};
use mixrelay_engine_core::{GraphResourceBackend, session::AdapterRuntimeStatus};
use mixrelay_render::{BlockProcessor, PeakMeterBank, ProcessorExchange, RenderPlan};

use std::{
    ptr,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[path = "linux_application_identity.rs"]
mod application_identity;
#[path = "linux_applications.rs"]
mod applications;
#[path = "linux_clock.rs"]
mod clock;
#[path = "linux_devices.rs"]
mod devices;
#[path = "linux_driver_monitor.rs"]
mod driver_monitor;
#[path = "linux_endpoints.rs"]
mod endpoints;
#[path = "linux_format.rs"]
mod format;
use mixrelay_linux_pipewire as native;
#[path = "linux_processing.rs"]
mod processing;
#[path = "linux_recovery.rs"]
mod recovery;
#[cfg(test)]
#[cfg(feature = "native-lab")]
#[path = "linux_resource_tests.rs"]
mod resource_tests;
#[path = "linux_source_mute.rs"]
mod source_mute;
use crate::processing_failure::Failure;
use applications::{CaptureSet as PipeWireCaptureSet, CaptureWatch};
use devices::{DeviceStreams, DeviceWatch};
use endpoints::{EndpointControls, EndpointEdit, EndpointSet, EndpointSpecs};
use mixrelay_audio_io::recovery::RecoveryMailbox;
use mixrelay_engine_core::resource_requirements::{DeviceRequirements, ResourceRequirements};
use native::NativeStream;
#[cfg(all(test, feature = "native-lab"))]
use processing::LinuxProcessState;
const ENGINE_BLOCK_FRAMES: u32 = CURRENT_TIMER_FORMAT.block_frames;
/// Describes the native `PipeWire` processing capabilities.
#[must_use]
pub fn adapter_profile() -> mixrelay_engine_core::session::AdapterProfile {
    mixrelay_engine_core::session::AdapterProfile {
        initial_format: mixrelay_audio_io::session::CURRENT_TIMER_FORMAT,
        capabilities: mixrelay_audio_io::session::AudioSessionCapabilities {
            backend: AudioBackendKind::PipeWire,
            supported_sample_rates: format::RATES.to_vec(),
            supported_block_frames: format::BLOCKS.to_vec(),
            can_reconfigure_live: true,
            hardware_input: true,
            hardware_output: true,
            virtual_endpoint: true,
            application_capture: true,
            sample_rate_conversion: true,
            drift_correction: false,
        },
        application_capture_rates: format::RATES.to_vec(),
        clock_authority: mixrelay_audio_io::session::ClockAuthority::AudioServer,
        transport_latency_frames: None,
        requires_observed_device_rate: false,
    }
}
const ENGINE_SAMPLE_RATE: u32 = CURRENT_TIMER_FORMAT.sample_rate;
const MAX_ENDPOINT_CHANNELS: u16 = 64;

const fn native_format(format: RenderFormat) -> native::StreamFormat {
    native::StreamFormat {
        sample_rate: format.sample_rate,
        block_frames: format.block_frames,
    }
}

/// Driver observations and revision-checked mutations consumed by native graph resources.
pub trait EndpointDriver: Send + Sync {
    /// # Errors
    /// Returns the original Driver transport or protocol failure.
    fn snapshot(
        &self,
    ) -> Result<mixrelay_driver_core::DriverSnapshot, mixrelay_driver_core::DriverError>;
    /// # Errors
    /// Returns stale revisions, unsupported formats or transport failure.
    fn mutate(
        &self,
        command: &mixrelay_driver_core::DriverCommand,
    ) -> Result<mixrelay_driver_core::DriverSnapshot, mixrelay_driver_core::DriverError>;
}

/// Creates status policy with this backend's independently owned endpoint observations.
#[must_use]
pub fn prepare_runtime(
    time: Arc<dyn mixrelay_engine_core::session::ProcessingTime>,
    observation: Arc<EndpointObservation>,
) -> AdapterRuntimeStatus {
    AdapterRuntimeStatus::new(adapter_profile(), time)
        .with_graph_format_control()
        .with_observed_rate_following(move || observation.rate())
}

pub use format::{EndpointObservation, Monitor as FormatMonitor};
pub use processing::{WorkerError, run_audio_worker};
pub use recovery::{DeviceRecoveryWorker, start as start_recovery};

/// Compiles the selected startup graph without publishing native resources.
/// # Errors
/// Returns graph compilation or DSP preparation failure.
pub fn create_initial_processor(
    document: &GraphDocument,
    meters: Arc<PeakMeterBank>,
    format: RenderFormat,
) -> Result<Box<BlockProcessor>, String> {
    let execution = GraphCompiler::default()
        .compile(
            document,
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .map_err(|error| error.to_string())?;
    let render = RenderPlan::prepare(&execution).map_err(|error| error.to_string())?;
    BlockProcessor::with_peak_meters(Arc::new(render), meters)
        .map(Box::new)
        .map_err(|error| error.to_string())
}

const fn render_format() -> RenderFormat {
    RenderFormat {
        sample_rate: ENGINE_SAMPLE_RATE,
        block_frames: ENGINE_BLOCK_FRAMES,
    }
}

/// Starts control-side reclamation for native resources and retired DSP plans.
/// # Errors
/// Returns thread creation failure without leaving a reclaimer detached.
pub fn spawn_reclaimers(
    exchange: &Arc<ProcessorExchange>,
    endpoints: &Arc<LinuxVirtualEndpointBackend>,
) -> Result<mixrelay_engine_host::resource_reclaimer::ResourceReclaimer, String> {
    let weak_exchange = Arc::downgrade(exchange);
    let weak_endpoints = Arc::downgrade(endpoints);
    mixrelay_engine_host::resource_reclaimer::ResourceReclaimer::start(
        "mixrelay-linux-reclaimer",
        move || {
            let Some(exchange) = weak_exchange.upgrade() else {
                return false;
            };
            exchange.reclaim_retired();
            drop(exchange);
            let Some(endpoints) = weak_endpoints.upgrade() else {
                return false;
            };
            endpoints.reclaim_retired();
            true
        },
    )
    .map_err(|error| format!("could not start Linux resource reclaimer: {error}"))
}

pub struct LinuxVirtualEndpointBackend {
    stream_owners: native::StreamOwners,
    driver: Arc<dyn EndpointDriver>,
    observation: Arc<EndpointObservation>,
    self_ref: Weak<Self>,
    active_format: AtomicU64,
    format_reserved: AtomicBool,
    capabilities: VirtualEndpointCapabilities,
    active_spec: Mutex<LinuxResourceSpec>,
    endpoint_controls: Mutex<EndpointControls>,
    pending: AtomicPtr<LinuxResourceUpdate>,
    retired: AtomicPtr<LinuxResourceUpdate>,
    device_generation: AtomicU64,
    device_watch: Mutex<Option<DeviceWatch>>,
    device_recovery: RecoveryMailbox<DeviceStreams>,
    application_generation: AtomicU64,
    application_watch: Mutex<Option<CaptureWatch>>,
    application_recovery: RecoveryMailbox<PipeWireCaptureSet>,
    // Weak, control-only access. None is initial setup, never shutdown fallback.
    graph_inputs: Mutex<Option<mixrelay_linux_pipewire::GraphInputs>>,
    playback: source_mute::PlaybackController,
    #[cfg(test)]
    automatic_device_outputs: bool,
}

impl LinuxVirtualEndpointBackend {
    /// Prepares this backend's resource scope without opening native streams.
    #[must_use]
    pub fn new(
        driver: Arc<dyn EndpointDriver>,
        format: RenderFormat,
        observation: Arc<EndpointObservation>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self::prepare(weak, driver, format, observation))
    }

    #[cfg(all(test, feature = "native-lab"))]
    fn new_standalone_device_publishers() -> Arc<Self> {
        Arc::new_cyclic(|self_ref| {
            let mut backend = Self::prepare(
                self_ref,
                test_driver(),
                render_format(),
                Arc::new(EndpointObservation::default()),
            );
            backend.automatic_device_outputs = true;
            backend
        })
    }

    fn prepare(
        self_ref: &std::sync::Weak<Self>,
        driver: Arc<dyn EndpointDriver>,
        format: RenderFormat,
        observation: Arc<EndpointObservation>,
    ) -> Self {
        Self {
            stream_owners: native::StreamOwners::default(),
            driver,
            observation,
            self_ref: self_ref.clone(),
            active_format: AtomicU64::new(format::pack(format)),
            format_reserved: AtomicBool::new(false),
            capabilities: VirtualEndpointCapabilities {
                max_render_channels: MAX_ENDPOINT_CHANNELS,
                max_capture_channels: MAX_ENDPOINT_CHANNELS,
                supported_sample_rates: format::RATES.to_vec(),
                supports_duplex: true,
                supports_dynamic_creation: true,
            },
            active_spec: Mutex::new(LinuxResourceSpec {
                endpoints: EndpointSpecs::new(),
                captures: Vec::new(),
                devices: DeviceRequirements::default(),
            }),
            endpoint_controls: Mutex::default(),
            pending: AtomicPtr::new(ptr::null_mut()),
            retired: AtomicPtr::new(ptr::null_mut()),
            device_generation: AtomicU64::new(0),
            device_watch: Mutex::new(None),
            device_recovery: RecoveryMailbox::default(),
            application_generation: AtomicU64::new(0),
            application_watch: Mutex::new(None),
            application_recovery: RecoveryMailbox::default(),
            graph_inputs: Mutex::new(None),
            playback: source_mute::PlaybackController::default(),
            #[cfg(test)]
            automatic_device_outputs: false,
        }
    }

    fn recover_applications(&self, stopping: &AtomicBool) {
        self.application_recovery.reclaim();
        if !self.application_recovery.idle() {
            return;
        }
        let watch = self
            .application_watch
            .lock()
            .ok()
            .and_then(|watch| watch.clone());
        let Some(watch) = watch else {
            return;
        };
        let Ok(Some((mut prepared, next_watch))) = watch.recover(stopping) else {
            return;
        };
        if stopping.load(Ordering::Acquire) {
            return;
        }
        if self.bind_application_inputs(&mut prepared).is_err() {
            return;
        }
        if let Ok(mut registered) = self.application_watch.lock()
            && registered
                .as_ref()
                .is_some_and(|current| current.generation == watch.generation)
            && self
                .application_recovery
                .publish(watch.generation, prepared)
                .is_ok()
            && let Some(current) = registered.as_mut()
        {
            current.refresh_leases(&next_watch);
        }
    }

    fn poll_source_mute(&self, exchange: &ProcessorExchange) {
        let watch = self
            .application_watch
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let spec = self.active_spec.lock().ok().map(|value| value.clone());
        if let Some(spec) = spec {
            let driver = if watch.is_some() && !spec.endpoints.is_empty() {
                self.driver_snapshot().ok()
            } else {
                None
            };
            self.playback.poll(
                exchange,
                &source_mute::PlaybackResources {
                    watch: watch.as_ref(),
                    endpoints: &spec.endpoints,
                    driver: driver.as_ref(),
                },
            );
        }
    }

    fn prepare_capture_edit(
        &self,
        requests: &[(String, ApplicationCaptureRequest)],
        format: RenderFormat,
    ) -> Result<(Box<PipeWireCaptureSet>, CaptureWatch), HostAudioBackendError> {
        let generation = self.application_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let previous = self
            .application_watch
            .lock()
            .map_err(|_| HostAudioBackendError::BackendFailure {
                message: "application recovery state is poisoned".into(),
            })?
            .clone();
        PipeWireCaptureSet::open_format(
            &self.stream_owners,
            requests,
            generation,
            format,
            previous.as_ref(),
        )
        .map(|(captures, watch)| (Box::new(captures), watch))
        .map_err(|message| HostAudioBackendError::BackendFailure { message })
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
            .map_err(|_| HostAudioBackendError::BackendFailure {
                message: "device recovery state is poisoned".into(),
            })?
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
            .map_err(|message| HostAudioBackendError::BackendFailure { message });
        }
        DeviceStreams::open_format(
            &self.stream_owners,
            spec,
            generation,
            format,
            previous.as_ref(),
        )
        .map(Box::new)
        .map_err(|message| HostAudioBackendError::BackendFailure { message })
    }

    fn activate_pending(
        &self,
        revision: u64,
        endpoints: &mut Box<EndpointSet>,
        captures: &mut Box<PipeWireCaptureSet>,
        devices: &mut Box<DeviceStreams>,
    ) -> Result<(), Failure> {
        if !self.retired.load(Ordering::Acquire).is_null() {
            return Err(Failure::RetirementPending);
        }
        let pending = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if pending.is_null() {
            return Ok(());
        }
        // SAFETY: `reconcile` creates this pointer and the audio thread atomically removed the
        // only stored owner. Destruction is deferred because it may wait for `pw-cli`.
        let mut update = unsafe { Box::from_raw(pending) };
        if update.revision != revision {
            self.pending.store(Box::into_raw(update), Ordering::Release);
            return Err(Failure::ResourceRevision);
        }
        if update
            .captures
            .as_ref()
            .is_some_and(|next| !next.can_retain_from(captures))
        {
            self.pending.store(Box::into_raw(update), Ordering::Release);
            return Err(Failure::RetainedCaptures);
        }
        if update
            .devices
            .as_ref()
            .is_some_and(|next| !next.can_retain_from(devices))
        {
            self.pending.store(Box::into_raw(update), Ordering::Release);
            return Err(Failure::RetainedDevices);
        }
        if update
            .endpoints
            .as_ref()
            .is_some_and(|next| !next.can_retain_from(endpoints))
        {
            self.pending.store(Box::into_raw(update), Ordering::Release);
            return Err(Failure::RetainedEndpoints);
        }
        if let Some(next) = &mut update.endpoints {
            next.retain_from(endpoints);
            std::mem::swap(endpoints, next);
        }
        if let Some(next) = &mut update.captures {
            next.retain_from(captures);
            std::mem::swap(captures, next);
        }
        if let Some(next) = &mut update.devices {
            next.retain_from(devices);
            std::mem::swap(devices, next);
        }
        // Reuse the prepared envelope to retire old resources. No Box allocation or
        // deallocation, process wait, or FIFO teardown occurs on this audio boundary.
        self.retired.store(Box::into_raw(update), Ordering::Release);
        Ok(())
    }

    fn reclaim_retired(&self) {
        let retired = self.retired.swap(ptr::null_mut(), Ordering::AcqRel);
        if !retired.is_null() {
            // SAFETY: the reclaimer removed the only pointer published by the audio thread.
            drop(unsafe { Box::from_raw(retired) });
        }
    }

    fn register_inputs(
        &self,
        inputs: &mixrelay_linux_pipewire::GraphInputs,
    ) -> Result<(), mixrelay_linux_pipewire::DriverError> {
        let mut registered = self.graph_inputs.lock().map_err(|_| {
            mixrelay_linux_pipewire::DriverError::Input("driver registration poisoned".into())
        })?;
        let pending = self.pending.load(Ordering::Acquire);
        if !pending.is_null() {
            // SAFETY: only the initial unpublished driver calls this hook. Its RT
            // DSP consumer has not started; graph_inputs excludes control publication.
            // The unique pending envelope remains owned by the existing mailbox.
            unsafe { &mut *pending }
                .bind_inputs(inputs)
                .map_err(mixrelay_linux_pipewire::DriverError::Input)?;
        }
        *registered = Some(inputs.clone());
        Ok(())
    }

    fn bind_application_inputs(&self, captures: &mut PipeWireCaptureSet) -> Result<(), String> {
        let registered = self
            .graph_inputs
            .lock()
            .map_err(|_| "driver registration poisoned")?;
        let inputs = registered.as_ref().ok_or("driver has not registered")?;
        inputs.ensure_live().map_err(|error| error.to_string())?;
        captures.bind_inputs(inputs)
    }

    fn bind_device_inputs(&self, devices: &mut DeviceStreams) -> Result<(), String> {
        let registered = self
            .graph_inputs
            .lock()
            .map_err(|_| "driver registration poisoned")?;
        let inputs = registered.as_ref().ok_or("driver has not registered")?;
        inputs.ensure_live().map_err(|error| error.to_string())?;
        devices.bind_inputs(inputs)
    }

    fn bind_format_inputs(&self, update: &mut LinuxResourceUpdate) -> Result<(), String> {
        let registered = self
            .graph_inputs
            .lock()
            .map_err(|_| "driver registration poisoned")?;
        let inputs = registered.as_ref().ok_or("driver has not registered")?;
        update.bind_inputs(inputs)
    }

    fn publish(&self, mut update: LinuxResourceUpdate) -> Result<(), HostAudioBackendError> {
        let registered =
            self.graph_inputs
                .lock()
                .map_err(|_| HostAudioBackendError::BackendFailure {
                    message: "driver registration poisoned".into(),
                })?;
        if let Some(inputs) = registered.as_ref() {
            update
                .bind_inputs(inputs)
                .map_err(|message| HostAudioBackendError::BackendFailure { message })?;
        }
        if self.format_reserved.load(Ordering::Acquire) {
            return Err(HostAudioBackendError::BackendFailure {
                message: "PipeWire format update reserved".into(),
            });
        }
        let pointer = Box::into_raw(Box::new(update));
        if self
            .pending
            .compare_exchange(
                ptr::null_mut(),
                pointer,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            // SAFETY: publication failed, so this thread retains the allocation.
            drop(unsafe { Box::from_raw(pointer) });
            return Err(HostAudioBackendError::BackendFailure {
                message: "a PipeWire endpoint update is already pending".to_owned(),
            });
        }
        Ok(())
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
        let controls =
            self.endpoint_controls
                .lock()
                .map_err(|_| HostAudioBackendError::BackendFailure {
                    message: "PipeWire endpoint control state is poisoned".into(),
                })?;
        EndpointEdit::prepare(&self.stream_owners, next, previous, &controls, format).map(Some)
    }

    fn publish_edit(
        &self,
        mut update: LinuxResourceUpdate,
        endpoints: Option<EndpointEdit>,
    ) -> Result<(), HostAudioBackendError> {
        let Some(endpoints) = endpoints else {
            return self.publish(update);
        };
        let mut controls =
            self.endpoint_controls
                .lock()
                .map_err(|_| HostAudioBackendError::BackendFailure {
                    message: "PipeWire endpoint control state is poisoned".into(),
                })?;
        *controls = endpoints.publish(|endpoints| {
            update.endpoints = Some(endpoints);
            self.publish(update)
        })?;
        Ok(())
    }
}

impl VirtualEndpointBackend for LinuxVirtualEndpointBackend {
    fn settings(&self) -> Option<mixrelay_audio_io::VirtualEndpointSettings> {
        self.driver_snapshot()
            .ok()
            .map(|snapshot| snapshot.settings)
    }

    fn sample_rate(
        &self,
        id: &mixrelay_audio_io::VirtualEndpointId,
    ) -> Result<u32, HostAudioBackendError> {
        self.driver_snapshot()?
            .settings
            .devices
            .into_iter()
            .find(|device| &device.spec.id == id)
            .and_then(|device| device.observed_sample_rate)
            .ok_or_else(|| driver_failure("Driver device rate unavailable"))
    }

    fn set_sample_rate(
        &self,
        id: &mixrelay_audio_io::VirtualEndpointId,
        rate: u32,
    ) -> Result<(), HostAudioBackendError> {
        let snapshot = self.driver_snapshot()?;
        let device = snapshot
            .settings
            .devices
            .iter()
            .find(|device| &device.spec.id == id)
            .ok_or_else(|| driver_failure("Driver device is missing"))?;
        let client = &self.driver;
        let mut selection = None;
        for (direction, formats) in [
            (
                mixrelay_driver_core::NativeDirection::Render,
                &device.native_formats.render,
            ),
            (
                mixrelay_driver_core::NativeDirection::Capture,
                &device.native_formats.capture,
            ),
        ] {
            let Some(current) = formats.current else {
                continue;
            };
            let desired = mixrelay_audio_io::NativeStreamFormat {
                sample_rate: rate,
                ..current
            };
            if !formats.supported.contains(&desired) {
                return Err(driver_failure(
                    "Driver does not support the requested native format",
                ));
            }
            if selection.is_none() {
                selection = Some((direction, desired));
            }
        }
        let (direction, format) =
            selection.ok_or_else(|| driver_failure("Driver format unavailable"))?;
        // The Driver's clock is shared by both native directions. One revision-checked
        // mutation updates their rate together, preserving each PCM representation.
        client
            .mutate(&mixrelay_driver_core::DriverCommand::SetFormat {
                expected_revision: snapshot.revision,
                endpoint_id: id.clone(),
                direction,
                format,
            })
            .map_err(driver_failure)?;
        Ok(())
    }

    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        &self.capabilities
    }

    fn prepare_graph_attachments(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.resolve_graph_devices(desired).map(|_| ())
    }

    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        if desired.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.driver_snapshot()?;
        desired
            .iter()
            .map(|reference| {
                snapshot
                    .settings
                    .devices
                    .iter()
                    .find(|device| device.spec.id == reference.id)
                    .map(|device| device.spec.clone())
                    .ok_or_else(|| {
                        driver_failure(format!(
                            "Driver device is missing: {}",
                            reference.id.as_str()
                        ))
                    })
            })
            .collect()
    }
}

fn driver_failure(error: impl std::fmt::Display) -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure {
        message: error.to_string(),
    }
}

impl LinuxVirtualEndpointBackend {
    fn driver_snapshot(
        &self,
    ) -> Result<mixrelay_driver_core::DriverSnapshot, HostAudioBackendError> {
        let snapshot = self.driver.snapshot().map_err(driver_failure)?;
        if snapshot.state != mixrelay_driver_core::DriverState::Ready {
            return Err(driver_failure("Driver service is not ready"));
        }
        Ok(snapshot)
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
        application_identity::resolve(request)
    }

    fn device_inventory(
        &self,
        _selectors: &[&mixrelay_audio_io::DeviceSelector],
    ) -> Result<Vec<mixrelay_audio_io::AudioDeviceDescriptor>, HostAudioBackendError> {
        mixrelay_linux_audio_io::devices()
    }

    fn prepare_format_change(
        &self,
        document: &mixrelay_audio_graph::GraphDocument,
        playback: mixrelay_engine_core::PlaybackBindings<'_>,
        _current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<Box<dyn mixrelay_engine_core::PreparedGraphFormat>, HostAudioBackendError> {
        let playback = source_mute::OwnedPlan::prepare(&playback)
            .map_err(|capability| HostAudioBackendError::CapabilityUnavailable { capability })?;
        format::prepare(self, document, requested, playback)
    }
    fn reconcile(
        &self,
        document: &mixrelay_audio_graph::GraphDocument,
        playback: mixrelay_engine_core::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        let playback = source_mute::OwnedPlan::prepare(&playback)
            .map_err(|capability| HostAudioBackendError::CapabilityUnavailable { capability })?;
        let next = LinuxResourceSpec::from_document(document)?;
        let format = format::unpack(self.active_format.load(Ordering::Acquire));
        let mut active =
            self.active_spec
                .lock()
                .map_err(|_| HostAudioBackendError::BackendFailure {
                    message: "PipeWire endpoint state is poisoned".to_owned(),
                })?;
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
        let mut registered =
            self.device_watch
                .lock()
                .map_err(|_| HostAudioBackendError::BackendFailure {
                    message: "device recovery state is poisoned".into(),
                })?;
        let mut registered_applications =
            self.application_watch
                .lock()
                .map_err(|_| HostAudioBackendError::BackendFailure {
                    message: "application recovery state is poisoned".into(),
                })?;
        if let (Some(prepared), Some(current)) =
            (application_watch.as_mut(), registered_applications.as_ref())
        {
            prepared.refresh_leases(current);
        }
        self.publish_edit(
            LinuxResourceUpdate {
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
        let pending = *self.pending.get_mut();
        if !pending.is_null() {
            // SAFETY: dropping the backend is exclusive; no shared owner can reach the update.
            drop(unsafe { Box::from_raw(pending) });
        }
        let retired = *self.retired.get_mut();
        if !retired.is_null() {
            // SAFETY: dropping the backend is exclusive; no shared owner can reach the endpoint.
            drop(unsafe { Box::from_raw(retired) });
        }
    }
}

#[derive(Clone, PartialEq)]
struct LinuxResourceSpec {
    endpoints: EndpointSpecs,
    captures: Vec<(String, ApplicationCaptureRequest)>,
    devices: DeviceRequirements,
}

impl LinuxResourceSpec {
    fn from_document(
        document: &mixrelay_audio_graph::GraphDocument,
    ) -> Result<Self, HostAudioBackendError> {
        let requirements = ResourceRequirements::from_document(document);
        Ok(Self {
            endpoints: endpoints::specs(requirements.endpoints)?,
            captures: requirements.captures,
            devices: requirements.devices,
        })
    }
}

struct LinuxResourceUpdate {
    revision: u64,
    endpoints: Option<Box<EndpointSet>>,
    captures: Option<Box<PipeWireCaptureSet>>,
    devices: Option<Box<DeviceStreams>>,
}

impl LinuxResourceUpdate {
    fn bind_inputs(&mut self, inputs: &mixrelay_linux_pipewire::GraphInputs) -> Result<(), String> {
        inputs.ensure_live().map_err(|error| error.to_string())?;
        if let Some(endpoints) = &mut self.endpoints {
            endpoints.bind_inputs(inputs)?;
        }
        if let Some(captures) = &mut self.captures {
            captures.bind_inputs(inputs)?;
        }
        if let Some(devices) = &mut self.devices {
            devices.bind_inputs(inputs)?;
        }
        Ok(())
    }
}

fn sanitize_node_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("io.mixrelay.{value}")
}

#[cfg(test)]
fn test_backend() -> Arc<LinuxVirtualEndpointBackend> {
    LinuxVirtualEndpointBackend::new(
        test_driver(),
        render_format(),
        Arc::new(EndpointObservation::default()),
    )
}

#[cfg(test)]
fn test_driver() -> Arc<dyn EndpointDriver> {
    #[cfg(feature = "native-lab")]
    struct Driver(mixrelay_driver_adapters::DriverClient);
    #[cfg(not(feature = "native-lab"))]
    struct Driver;
    impl EndpointDriver for Driver {
        fn snapshot(
            &self,
        ) -> Result<mixrelay_driver_core::DriverSnapshot, mixrelay_driver_core::DriverError>
        {
            #[cfg(feature = "native-lab")]
            return self.0.snapshot_blocking();
            #[cfg(not(feature = "native-lab"))]
            Err(mixrelay_driver_core::DriverError::Unavailable(
                "fixture has no Driver".into(),
            ))
        }
        fn mutate(
            &self,
            command: &mixrelay_driver_core::DriverCommand,
        ) -> Result<mixrelay_driver_core::DriverSnapshot, mixrelay_driver_core::DriverError>
        {
            #[cfg(feature = "native-lab")]
            return self.0.mutate_blocking(command);
            #[cfg(not(feature = "native-lab"))]
            {
                let _ = command;
                Err(mixrelay_driver_core::DriverError::Unavailable(
                    "fixture has no Driver".into(),
                ))
            }
        }
    }
    #[cfg(feature = "native-lab")]
    return Arc::new(Driver(
        mixrelay_driver_adapters::DriverClient::new().unwrap(),
    ));
    #[cfg(not(feature = "native-lab"))]
    Arc::new(Driver)
}
