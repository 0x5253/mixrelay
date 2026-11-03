//! `CoreAudio` resource coordination. Graph publication, recovery and format
//! transactions have separate activation contracts; no native work runs on DSP.
mod activity;
mod admission;
mod aggregate;
mod application;
mod bridge;
mod buffer_control;
mod buffer_transaction;
#[cfg(test)]
pub(crate) mod callback_fixture;
#[cfg(any(test, feature = "test-util"))]
mod callback_invoke;
#[cfg(any(test, feature = "test-util"))]
mod controller_fixture;
mod device;
pub(super) mod direct;
mod ffi;
mod format;
mod group_plan;
mod handoff;
mod host;
pub(super) mod latency;
mod native;
mod permission;
mod recovery;
mod retirement;
#[cfg(any(test, feature = "test-util"))]
mod simulated;
mod spec;
mod telemetry;
mod worker;
mod worker_retirement;
use aggregate::Aggregate;
#[cfg(test)]
use bridge::DeviceBridge;
use device::Device;
use ffi::mr_device_io_native_frames;
#[cfg(test)]
use ffi::*;
use format::{PreparedDeviceFormat, PublishedFormat};
use handoff::{Publication, ResourceConsumer, ResourceHandoff};
use mixrelay_audio_graph::{GraphDocument, RenderFormat};
use mixrelay_audio_io::{HostAudioBackendError, process::TransferOutcome};
use mixrelay_engine::{
    GraphResourceBackend, PreparedGraphFormat, ProcessingControlBackend, RuntimeStatusBackend,
    SessionFormatBackend, session::AdapterRuntimeStatus,
};
use spec::{Spec, aggregate_members, specs};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[cfg(test)]
use {
    bridge::NEXT_DEVICE_EPOCH,
    mixrelay_audio_graph::{DeviceSelector, NodeConfig},
    native::Native,
    std::{
        sync::atomic::{AtomicU32, AtomicU64},
        thread,
    },
    worker::WorkerContext,
};
#[derive(Default)]
pub(super) struct DeviceSet(BTreeMap<String, Arc<Device>>, BTreeMap<String, Spec>);
pub(super) struct HardwareClockProgress {
    pub(super) owner_id: u64,
    pub(super) epoch: u64,
    pub(super) native_frames: u64,
    pub(super) native_rate: u32,
    pub(super) quantum: u32,
}
impl DeviceSet {
    pub(super) fn reserve_capture_alignment(&self, rate: u32, frames: u32, enabled: bool) {
        // The sinc bridge publishes 128-frame slabs. Retain one further slab
        // beyond producer/consumer bursts; until observed, budget two slabs.
        const CONVERTER_SLAB: u32 = 128;
        let burst = self
            .0
            .values()
            .filter(|device| device.input && device.ready() && device.activity.enabled())
            .fold(frames.max(CONVERTER_SLAB * 2), |largest, device| {
                // SAFETY: this set retains the native owner; geometry is atomic.
                let native =
                    unsafe { ffi::mr_device_io_quantum(device.native.pointer.as_ptr(), false) };
                let native_rate = device.native_rate.load(Ordering::Acquire).max(1);
                let mapped = (u64::from(native) * u64::from(rate)).div_ceil(u64::from(native_rate));
                // SAFETY: this is the serialized graph consumer; successive native
                // IOProcs may have published several slabs before this graph cycle.
                let published = unsafe {
                    mixrelay_macos_transport::device_io::capture_burst(
                        device.native.pointer.as_ptr().cast(),
                        device
                            .activity
                            .read_epoch(device.epoch.load(Ordering::Acquire)),
                    )
                };
                largest
                    .max(u32::try_from(mapped).unwrap_or(u32::MAX))
                    .max(published)
            });
        for device in self.0.values().filter(|device| device.input) {
            // SAFETY: atomic reserve update; callback retains all mutable conversion state.
            unsafe {
                mixrelay_macos_transport::device_io::set_capture_window(
                    device.native.pointer.as_ptr().cast(),
                    if enabled {
                        burst
                            .saturating_add(frames)
                            .saturating_add(CONVERTER_SLAB)
                            .saturating_mul(2)
                    } else {
                        0
                    },
                );
            }
        }
    }
    pub(super) fn binding_present(&self, id: &str, input: bool) -> bool {
        self.binding_key(id, input)
            .and_then(|key| self.0.get(key))
            .is_some_and(|device| device.ready())
    }
    pub(super) fn capture_time(&self, id: &str, frames: u32) -> Option<u64> {
        let id = self.binding_key(id, true)?;
        let device = self.0.get(id)?;
        if !device.ready() || !device.activity.ready() {
            return None;
        }
        // SAFETY: this set retains the input owner; the serialized graph consumer
        // acknowledges its PCM epoch before observing the matching coordinates.
        unsafe {
            mixrelay_macos_transport::device_io::input_time(
                device.native.pointer.as_ptr().cast(),
                frames,
            )
        }
    }

    pub(super) fn frame_period(&self, rate: u32) -> Option<f64> {
        let device = self.clock_master()?;
        // SAFETY: the set retains the native pacing owner during this atomic observation.
        let native = unsafe {
            mixrelay_macos_transport::device_io::frame_nanoseconds(
                device.native.pointer.as_ptr().cast(),
            )
        };
        (native.is_finite() && native > 0.0).then(|| {
            native
                * f64::from(
                    device
                        .native_rate
                        .load(std::sync::atomic::Ordering::Acquire),
                )
                / f64::from(rate)
        })
    }

    pub(super) fn read_aligned(
        &self,
        id: &str,
        samples: &mut [f32],
        timing: (u64, u64, u32),
    ) -> Option<TransferOutcome> {
        let id = self.binding_key(id, true)?;
        if let Some(spec) = self.1.get(id) {
            samples.fill(0.0);
            return Some(recovery::offline_transfer(
                samples.len(),
                usize::from(spec.channels),
            ));
        }
        self.0
            .get(id)
            .map(|device| device.read_timed(samples, timing.0, Some((timing.1, timing.2))))
    }
    /// Publish the pacing callback's complete burst, not each split DSP block.
    pub(super) fn set_graph_quantum(&self, frames: u32) {
        for device in self.0.values() {
            device.graph_quantum.store(frames, Ordering::Release);
        }
    }
    fn binding_key(&self, id: &str, input: bool) -> Option<&str> {
        if let Some((key, _)) = self.0.get_key_value(id) {
            return Some(key);
        }
        if let Some((key, _)) = self.1.get_key_value(id) {
            return Some(key);
        }
        let suffix = if input { "/capture" } else { "/render" };
        let bounds = (std::ops::Bound::Included(id), std::ops::Bound::Unbounded);
        self.0
            .range::<str, _>(bounds)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(id))
            .chain(
                self.1
                    .range::<str, _>(bounds)
                    .map(|(key, _)| key)
                    .take_while(|key| key.starts_with(id)),
            )
            .find(|key| key.strip_suffix(suffix) == Some(id))
            .map(String::as_str)
    }

    /// Applies the compiled binding gate to only its capture or render client.
    pub(super) fn set_binding_active(&self, id: &str, input: bool, enabled: bool) {
        if let Some(key) = self.binding_key(id, input)
            && let Some(device) = self.0.get(key)
        {
            device.activity.admit();
            device.set_active(enabled);
        }
    }

    pub(super) fn is_offline(&self, id: &str) -> bool {
        self.1.contains_key(id)
    }

    /// Observes disconnected/disabled clients without changing device identity.
    pub(super) fn is_binding_inactive(&self, id: &str, input: bool) -> bool {
        self.binding_key(id, input).is_some_and(|key| {
            self.1.contains_key(key)
                || self
                    .0
                    .get(key)
                    .is_some_and(|device| !device.ready() || !device.activity.enabled())
        })
    }
    /// Explicit graph pacing identity, never inferred from nominal sample rate.
    pub(super) fn set_clock_master(&self, owner_id: Option<u64>) {
        let master = self.0.values().find(|device| {
            owner_id == Some(device.identity) && !device.failed.load(Ordering::Acquire)
        });
        let master_clock = master.map_or(0, |device| device.clock_identity.load(Ordering::Acquire));
        let master_rate = master.map_or(0, |device| device.native_rate.load(Ordering::Acquire));
        for device in self.0.values() {
            // Two IOProcs of the same non-aggregate device share its timeline.
            // A shared nominal rate or unknown clock domain is never proof.
            // User aggregates stay independent until their subdevice policy is
            // verified; merely sharing an Aggregate object ID is insufficient.
            let aligned = master.is_some()
                && !device.failed.load(Ordering::Acquire)
                && (owner_id == Some(device.identity)
                    || (master_clock != 0
                        && device.clock_identity.load(Ordering::Acquire) == master_clock
                        && master_rate != 0
                        && device.native_rate.load(Ordering::Acquire) == master_rate));
            device.clock_aligned.store(aligned, Ordering::Release);
        }
    }
    /// Reports readiness of one direction's converter, or no native owner.
    pub(super) fn clock_synchronized(&self, id: &str, input: bool) -> Option<bool> {
        self.binding_key(id, input)
            .and_then(|key| self.0.get(key))
            .map(|device| device.ready() && device.bridge_ready.load(Ordering::Acquire))
    }
    /// A pacing candidate is not proof that unrelated clocks are synchronized.
    /// Prefer output demand; capture-only graphs can use a healthy input clock.
    pub(super) fn hardware_clock_progress(&self) -> Option<HardwareClockProgress> {
        self.clock_master().map(|device| HardwareClockProgress {
            owner_id: device.identity,
            epoch: device.epoch.load(Ordering::Acquire),
            native_frames: unsafe { mr_device_io_native_frames(device.native.pointer.as_ptr()) },
            native_rate: device.native_rate.load(Ordering::Acquire),
            // SAFETY: the device set retains Native; quantum is an atomic snapshot.
            quantum: unsafe { ffi::mr_device_io_quantum(device.native.pointer.as_ptr(), false) },
        })
    }

    fn clock_master(&self) -> Option<&Device> {
        self.0
            .values()
            .map(Arc::as_ref)
            .filter(|device| {
                !device.failed.load(Ordering::Acquire)
                    && device.native_rate.load(Ordering::Acquire) != 0
            })
            .min_by_key(|device| device.input)
    }

    /// Do not render a device-paced plan using a saved or previous clock rate.
    /// Independent secondary devices may still need conversion; only the actual
    /// pacing owner's rate determines the plan. Offline branches do not gate it.
    pub(super) fn processing_ready(&self, sample_rate: u32) -> bool {
        self.clock_master().is_none_or(|device| {
            device.ready()
                && device.graph_rate.load(Ordering::Acquire) == sample_rate
                && device.native_rate.load(Ordering::Acquire) == sample_rate
        })
    }
    /// The pacing device's latest callback label. A graph cycle labels all its reads and
    /// writes with this one time, so the label cancels between a route's source and sink
    /// FIFOs and their passages sum to the frame's actual bridge-to-bridge delay.
    pub(super) fn graph_host(&self) -> u64 {
        self.clock_master()
            .map(Device::callback_host)
            .filter(|host| *host != 0)
            .unwrap_or_else(mixrelay_realtime::host_time)
    }
    pub(super) fn read(&self, id: &str, samples: &mut [f32], host: u64) -> Option<TransferOutcome> {
        let id = self.binding_key(id, true)?;
        if let Some(spec) = self.1.get(id) {
            samples.fill(0.0);
            return Some(recovery::offline_transfer(
                samples.len(),
                usize::from(spec.channels),
            ));
        }
        self.0.get(id).map(|device| device.read(samples, host))
    }
    pub(super) fn write(&self, id: &str, samples: &[f32], host: u64) -> Option<TransferOutcome> {
        let id = self.binding_key(id, false)?;
        if let Some(spec) = self.1.get(id) {
            return Some(recovery::offline_transfer(
                samples.len(),
                usize::from(spec.channels),
            ));
        }
        if let Some(device) = self.0.get(id)
            && !device.ready()
        {
            return Some(recovery::offline_transfer(samples.len(), device.channels));
        }
        self.0.get(id).map(|device| device.write(samples, host))
    }
}
type Cache = BTreeMap<String, (Spec, Weak<Device>)>;
pub struct MacDevices {
    cache: Arc<Mutex<Cache>>,
    handoff: Arc<ResourceHandoff<DeviceSet>>,
    status: Arc<AdapterRuntimeStatus>,
    format: Arc<PublishedFormat>,
    next_group_check: Mutex<Instant>,
    recovery_requested: AtomicBool,
    stopping: AtomicBool,
    prepared_hardware_clock: Arc<AtomicBool>,
    hardware_clock: AtomicBool,
    host: Arc<dyn host::Host>,
    device_credit: Option<Arc<mixrelay_realtime::Notifier>>,
}
impl MacDevices {
    /// All graph/device owners must be dropped before this final process drain.
    /// # Errors
    /// Returns retained native worker teardown failures after joining every owner.
    pub fn finish_retirement() -> Result<(), String> {
        let result = worker_retirement::WORKERS.finish();
        while Aggregate::active_owners() != 0 {
            Aggregate::reclaim_retired();
            std::thread::sleep(Duration::from_millis(100));
        }
        result
    }
    pub(super) fn has_pending(&self) -> bool {
        self.handoff.has_pending()
    }
    pub fn new(status: Arc<AdapterRuntimeStatus>, rate: u32, buffer_frames: u32) -> Self {
        let wake = mixrelay_realtime::Notifier::new()
            .inspect_err(|error| tracing::warn!(%error, "graph worker polls for device credit"))
            .ok()
            .map(Arc::new);
        let signal = wake.as_ref().map(|wake| {
            let wake = Arc::clone(wake);
            Arc::new(move || wake.notify()) as Arc<dyn Fn() + Send + Sync>
        });
        let mut devices = Self::with_host(
            status,
            rate,
            buffer_frames,
            Arc::new(host::CoreAudio::with_credit_signal(signal)),
        );
        devices.device_credit = wake;
        devices
    }
    /// A coordinator over a simulated `CoreAudio` that publishes no device, for tests that
    /// exercise coordination without devices. Nothing reaches the machine's HAL.
    #[cfg(test)]
    pub(super) fn without_devices(
        status: Arc<AdapterRuntimeStatus>,
        rate: u32,
        buffer_frames: u32,
    ) -> Self {
        Self::with_host(status, rate, buffer_frames, simulated::SimulatedHost::new())
    }
    fn with_host(
        status: Arc<AdapterRuntimeStatus>,
        rate: u32,
        buffer_frames: u32,
        host: Arc<dyn host::Host>,
    ) -> Self {
        Self {
            cache: Arc::default(),
            handoff: Arc::default(),
            status,
            format: Arc::new(PublishedFormat::new(rate, buffer_frames)),
            next_group_check: Mutex::new(Instant::now()),
            recovery_requested: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            prepared_hardware_clock: Arc::default(),
            hardware_clock: AtomicBool::new(false),
            host,
            device_credit: None,
        }
    }
    pub(crate) fn device_wakeup(&self) -> Option<Arc<mixrelay_realtime::Notifier>> {
        self.device_credit.clone()
    }

    pub fn reclaim(&self) {
        worker_retirement::WORKERS.reclaim();
        let Ok(_cache) = self.cache.try_lock() else {
            return;
        };
        self.handoff.reclaim();
        Aggregate::reclaim_retired();
    }

    /// New native work belongs to the stoppable observer, never the reclaimer
    /// that must remain alive while the audio worker is being joined.
    pub fn recover(&self, stop: &AtomicBool) {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut cache) = self.cache.try_lock() else {
            return;
        };
        // Membership, rate changes and hotplug are reconciled off the DSP
        // thread. Preparation is transactional: old owners remain published
        // until an entire replacement set is ready, and invalid callbacks mute.
        let Ok(mut next) = self.next_group_check.try_lock() else {
            return;
        };
        if stop.load(Ordering::Acquire)
            || !self.handoff.idle()
            || (Instant::now() < *next && !self.recovery_requested.load(Ordering::Acquire))
        {
            return;
        }
        self.recovery_requested.store(false, Ordering::Release);
        *next = Instant::now() + Duration::from_secs(2);
        let desired = cache
            .iter()
            .map(|(id, (spec, _))| (id.clone(), spec.clone()))
            .collect();
        if let Err(error) = self.prepare_devices(&mut cache, &desired, Publication::Recovery) {
            tracing::warn!(%error, "CoreAudio private-group recovery deferred");
        }
    }
    /// Bypass the periodic inventory delay after callback topology loss.
    /// No native property access or resource construction occurs on the caller.
    pub(super) fn request_recovery(&self) {
        self.recovery_requested.store(true, Ordering::Release);
    }

    /// Cancels further preparation, without interrupting an in-flight native call or teardown.
    pub fn stop_preparing(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    fn require_preparation(&self) -> Result<(), HostAudioBackendError> {
        if self.stopping.load(Ordering::Acquire) {
            Err(backend_error(
                "CoreAudio preparation cancelled during shutdown",
            ))
        } else {
            Ok(())
        }
    }
    pub(super) fn activation(&self) -> Result<DeviceActivation<'_>, &'static str> {
        Ok(DeviceActivation {
            consumer: self
                .handoff
                .try_consumer()
                .ok_or("CoreAudio device activation already has a consumer")?,
            devices: self,
        })
    }

    pub fn follows_hardware(&self) -> bool {
        self.hardware_clock.load(Ordering::Acquire)
    }

    pub fn routing_rate(&self) -> Option<u32> {
        if !self.follows_hardware() {
            return None;
        }
        self.cache
            .lock()
            .ok()?
            .values()
            .filter_map(|(_, owner)| owner.upgrade())
            .filter(|device| !device.failed.load(Ordering::Acquire))
            .min_by_key(|device| device.input)
            .map(|device| device.native_rate.load(Ordering::Acquire))
            .filter(|rate| *rate != 0)
    }
}

/// Borrows one backend for a worker's entire activation lifetime. Keeping the
/// backend with its lane tokens prevents activating a different backend's state.
pub(super) struct DeviceActivation<'a> {
    consumer: ResourceConsumer<'a, DeviceSet>,
    devices: &'a MacDevices,
}

impl DeviceActivation<'_> {
    pub(super) fn activate(&mut self, active: &mut DeviceSet) {
        self.consumer.activate(Publication::Graph, active);
        self.devices.hardware_clock.store(
            self.devices.prepared_hardware_clock.load(Ordering::Acquire),
            Ordering::Release,
        );
    }

    /// Same-spec recovery is independent of graph revisions, at a block boundary.
    pub(super) fn activate_recovered(&mut self, active: &mut DeviceSet) {
        self.consumer.activate(Publication::Recovery, active);
    }
}
fn backend_error(message: impl Into<String>) -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure {
        message: message.into(),
    }
}
impl GraphResourceBackend for MacDevices {
    fn execution_released(&self) -> bool {
        native::Native::active_owners() == 0 && Aggregate::active_owners() == 0
    }
    fn device_inventory(
        &self,
        selectors: &[&mixrelay_audio_io::DeviceSelector],
    ) -> Result<Vec<mixrelay_audio_io::AudioDeviceDescriptor>, HostAudioBackendError> {
        self.require_preparation()?;
        self.host.inventory(selectors)
    }

    fn resolve_application_capture(
        &self,
        request: &mixrelay_audio_io::ApplicationCaptureRequest,
    ) -> Result<mixrelay_audio_graph::composition::SharedSource, HostAudioBackendError> {
        application::resolve(request)
    }

    fn prepare_format_change(
        &self,
        document: &GraphDocument,
        playback: mixrelay_engine::PlaybackBindings<'_>,
        current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        application::validate_playback(&playback)?;
        self.require_preparation()?;
        let cache = self
            .cache
            .lock()
            .map_err(|_| backend_error("CoreAudio device cache poisoned"))?;
        self.handoff.reclaim();
        let reservation = self.handoff.reserve_format().ok_or_else(|| {
            backend_error("CoreAudio resource or format update is already pending")
        })?;
        let (devices, next_cache) = self.prepare_device_set(&cache, specs(document), requested)?;
        self.require_preparation()?;
        let format = PreparedDeviceFormat::prepare(
            devices.0.values().cloned(),
            self.format.clone(),
            current,
            requested,
            reservation,
        )?;
        Ok(Box::new(format::PreparedDeviceReplacement {
            format,
            devices,
            next_cache,
            cache: self.cache.clone(),
            handoff: self.handoff.clone(),
            hardware_clock: self.prepared_hardware_clock.clone(),
        }))
    }
    fn reconcile(
        &self,
        document: &GraphDocument,
        playback: mixrelay_engine::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        application::validate_playback(&playback)?;
        self.require_preparation()?;
        let desired = specs(document);
        // Adding a virtual endpoint does not transfer hardware's clock authority
        // to that endpoint. Cross-clock transport is an explicit bridge boundary.
        let follow_hardware = !desired.is_empty();
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| backend_error("CoreAudio device cache poisoned"))?;
        if desired.len() == cache.len()
            && desired.iter().all(|(id, spec)| {
                cache
                    .get(id)
                    .is_some_and(|(old, device)| old == spec && device.strong_count() != 0)
            })
        {
            self.prepared_hardware_clock
                .store(follow_hardware, Ordering::Release);
            return Ok(());
        }
        self.handoff.reclaim();
        if !self.handoff.idle() {
            return Err(backend_error("CoreAudio device update is already pending"));
        }
        self.prepare_devices(&mut cache, &desired, Publication::Graph)?;
        self.prepared_hardware_clock
            .store(follow_hardware, Ordering::Release);
        Ok(())
    }
}
impl RuntimeStatusBackend for MacDevices {
    fn capabilities_for_format(
        &self,
        format: RenderFormat,
    ) -> Option<mixrelay_audio_io::session::AudioSessionCapabilities> {
        self.status
            .observe_routing_clock(self.follows_hardware(), self.routing_rate());
        self.status.capabilities_for_format(format)
    }
    fn awaiting_microphone_permission(&self) -> bool {
        self.host.permission_waiting()
    }
    fn status(&self, format: RenderFormat) -> mixrelay_control_protocol::AppStatus {
        self.status
            .observe_routing_clock(self.follows_hardware(), self.routing_rate());
        self.status.status(format)
    }
}

impl ProcessingControlBackend for MacDevices {
    fn fail_reconfiguration(&self) {
        self.status.fail_reconfiguration();
    }
    fn suspend(&self) -> Result<(), HostAudioBackendError> {
        self.status.pause_at_boundary()
    }
    fn resume(&self) {
        self.status.resume();
    }
}

impl SessionFormatBackend for MacDevices {
    fn external_sample_rate(&self) -> Option<u32> {
        self.status.external_sample_rate()
    }
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        if self.follows_hardware() {
            return self.routing_rate().map(|sample_rate| RenderFormat {
                sample_rate,
                ..current
            });
        }
        self.status.external_format(current)
    }
    fn adopt_external_format(&self, requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        if self.follows_hardware() {
            if self.routing_rate() != Some(requested.sample_rate) {
                return Err(backend_error("routing clock changed during preparation"));
            }
            self.status.adopt_routing_clock(requested)?;
            if self.routing_rate() != Some(requested.sample_rate) {
                return Err(backend_error("routing clock changed during suspension"));
            }
            return Ok(());
        }
        self.status.adopt_external_format(requested)
    }
    fn reconfigure(
        &self,
        current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<(), HostAudioBackendError> {
        if self.follows_hardware() {
            if self.routing_rate() != Some(requested.sample_rate) {
                return Err(backend_error(
                    "hardware routing follows its device sample rate; change the selected device format, not the unrelated virtual endpoint",
                ));
            }
            return self.status.reconfigure_routing_buffer(requested);
        }
        self.status.reconfigure(current, requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplex_driver_binding_lookup_keeps_capture_and_render_widths_without_rt_allocation() {
        let selector = DeviceSelector::PersistentUid {
            uid: mixrelay_macos_transport::endpoint_identity::endpoint_uid([1; 16]),
        };
        let devices = DeviceSet(
            BTreeMap::new(),
            BTreeMap::from([
                (
                    "driver/capture".into(),
                    Spec {
                        selector: selector.clone(),
                        input: true,
                        channels: 3,
                    },
                ),
                (
                    "driver/render".into(),
                    Spec {
                        selector,
                        input: false,
                        channels: 7,
                    },
                ),
            ]),
        );
        let mut input = [9.0; 96];
        let output = [0.5; 224];
        let allocation = allocation_counter::measure(|| {
            assert_eq!(devices.binding_key("driver", true), Some("driver/capture"));
            assert_eq!(devices.binding_key("driver", false), Some("driver/render"));
            assert_eq!(devices.read("driver", &mut input, 0).unwrap().frames, 32);
            assert_eq!(devices.write("driver", &output, 0).unwrap().frames, 32);
            assert!(input.iter().all(|sample| *sample == 0.0));
            assert_eq!(devices.read("missing", &mut input, 0), None);
            assert_eq!(devices.write("missing", &output, 0), None);
        });
        assert_eq!(allocation.count_total, 0);
    }

    #[test]
    fn virtual_device_buffer_follows_the_session_block() {
        let driver = Native::create(
            &Spec {
                selector: DeviceSelector::PersistentUid {
                    uid: mixrelay_macos_transport::endpoint_identity::endpoint_uid([1; 16]),
                },
                input: false,
                channels: 2,
            },
            48_000,
            None,
            32,
            simulated::SimulatedHost::new(),
        )
        .unwrap();
        assert_eq!(driver.requested_buffer(), 32);
        driver.request_buffer(256);
        assert_eq!(driver.requested_buffer(), 256);
    }
    #[test]
    fn processing_admission_waits_for_native_rate_and_rebuilt_bridge() {
        let (device, context) = synthetic_output(2, 96_000, 48_000);
        let devices = DeviceSet(BTreeMap::from([("output".into(), device)]), BTreeMap::new());
        assert!(
            !devices.processing_ready(96_000),
            "saved rate is not native proof"
        );
        assert!(
            !devices.processing_ready(48_000),
            "plan must match the prepared bridge"
        );
        context.graph_rate.store(48_000, Ordering::Release);
        assert!(
            !devices.processing_ready(48_000),
            "old bridge must not activate"
        );
        context.ready_rate.store(48_000, Ordering::Release);
        assert!(devices.processing_ready(48_000));
        context.native_rate.store(96_000, Ordering::Release);
        assert!(
            !devices.processing_ready(48_000),
            "reconnection must revalidate rate"
        );
        context.graph_rate.store(96_000, Ordering::Release);
        context.ready_rate.store(0, Ordering::Release);
        assert!(!devices.processing_ready(96_000));
        context.ready_rate.store(96_000, Ordering::Release);
        assert!(devices.processing_ready(96_000));
    }

    #[test]
    fn admission_keeps_independent_secondary_conversion_and_offline_branches() {
        let (master, _) = synthetic_output(2, 48_000, 48_000);
        let (secondary, secondary_context) = synthetic_output(2, 48_000, 44_100);
        let devices = DeviceSet(
            BTreeMap::from([("a-output".into(), master), ("b-output".into(), secondary)]),
            BTreeMap::new(),
        );
        assert!(devices.processing_ready(48_000));
        secondary_context.failed.store(true, Ordering::Release);
        assert!(devices.processing_ready(48_000));
        assert!(DeviceSet::default().processing_ready(96_000));
    }

    #[test]
    fn native_owner_allocation_retirement_stress() {
        // Lazily registered diagnostics callsites allocate once, outside the measurement.
        drop(synthetic_output(1, 48_000, 48_000));
        // All 512 owners exceed the 256-owner admission bound: every create below succeeds
        // only because each retirement returned its slot. Allocation is counted on this one
        // thread, which creates and destroys every owner, so concurrent tests cannot perturb it.
        let allocation = allocation_counter::measure(|| {
            for channels in [1, 2, 8, 64] {
                for _ in 0..128 {
                    let (device, context) = synthetic_output(channels, 48_000, 48_000);
                    let native = Arc::downgrade(&device.native);
                    drop(device);
                    assert!(
                        native.upgrade().is_some(),
                        "worker still owns native queues"
                    );
                    drop(context);
                    assert!(
                        native.upgrade().is_none(),
                        "last owner releases both queues and admission"
                    );
                }
            }
        });
        assert!(allocation.count_total > 0, "owners allocate their queues");
        assert_eq!(
            (allocation.count_current, allocation.bytes_current),
            (0, 0),
            "native create/retire cycles leaked"
        );
    }
    #[test]
    fn aggregate_members_are_unique_fixed_uids_with_output_clock_first() {
        let mut desired = BTreeMap::new();
        for (name, uid, input) in [("a", "mic", true), ("b", "out", false), ("c", "out", true)] {
            desired.insert(
                name.to_owned(),
                Spec {
                    selector: DeviceSelector::PersistentUid {
                        uid: uid.to_owned(),
                    },
                    input,
                    channels: 2,
                },
            );
        }
        desired.insert(
            "default".into(),
            Spec {
                selector: DeviceSelector::DefaultOutput,
                input: false,
                channels: 2,
            },
        );
        assert_eq!(aggregate_members(&desired), ["out", "mic"]);
    }
    #[test]
    fn queue_retains_complete_converter_bursts_without_padding_same_clock_copy() {
        let (device, context) = synthetic_output(2, 48000, 48000);
        let pcm = [0.0; 64];
        unsafe {
            mr_device_io_graph_write(device.native.pointer.as_ptr(), pcm.as_ptr(), 32, 0);
        }
        context.clock_aligned.store(true, Ordering::Release);
        let bridge = DeviceBridge::new(&context, 48000).unwrap();
        assert_eq!(bridge.queue_target(&context, false), 32);
        context.clock_aligned.store(false, Ordering::Release);
        let bridge = DeviceBridge::new(&context, 48000).unwrap();
        assert_eq!(
            bridge.queue_target(&context, false),
            bridge.bridge.processing_quantum().1 * 2
        );
    }
    #[test]
    fn output_queue_latency_budget() {
        let (device, context) = synthetic_output(2, 48000, 48000);
        context.clock_aligned.store(true, Ordering::Release);
        let mut bridge = DeviceBridge::new(&context, 48000).unwrap();
        let source = vec![0.125_f32; 256 * 2];
        for _ in 0..16 {
            assert_eq!(
                unsafe {
                    mr_device_io_graph_write(
                        device.native.pointer.as_ptr(),
                        source.as_ptr(),
                        256,
                        0,
                    )
                },
                256
            );
        }
        for _ in 0..32 {
            bridge.pump(&context).unwrap();
        }
        let queued = unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), false) };
        eprintln!(
            "same-clock 48kHz stereo native output queued={queued} frames ({:.3} ms)",
            f64::from(queued) / 48.0
        );
        assert_eq!(queued, 256);
    }

    #[test]
    fn split_pacing_burst_retains_the_complete_callback_not_one_dsp_block() {
        for (native_rate, aligned) in [(48_000, true), (96_000, false)] {
            let mut transferred = Vec::new();
            for pacing_quantum in [32, 256] {
                let (device, context) = synthetic_device(1, 48_000, native_rate, true);
                context.clock_aligned.store(aligned, Ordering::Release);
                let devices = DeviceSet(
                    BTreeMap::from([("input".into(), device.clone())]),
                    BTreeMap::new(),
                );
                devices.set_graph_quantum(pacing_quantum);
                let mut bridge = DeviceBridge::new(&context, native_rate).unwrap();
                // Establish the graph consumer epoch before queuing this callback.
                let mut pcm = [0.0; 32];
                device.read(&mut pcm, 0);
                for _ in 0..4 {
                    bridge.bridge.push_interleaved(&[0.25; 256]).unwrap();
                }
                // Let sinc startup consume its filter history before checking
                // the retained steady-state transfer window.
                for _ in 0..4 {
                    bridge.pump(&context).unwrap();
                }
                let frames: u32 = (0..8).map(|_| device.read(&mut pcm, 0).frames).sum();
                let expected = if aligned {
                    pacing_quantum
                } else {
                    (pacing_quantum * 2)
                        .max(u32::try_from(bridge.bridge.processing_quantum().1 * 2).unwrap())
                }
                .min(256)
                    / 32
                    * 32;
                assert_eq!(frames, expected);
                transferred.push(frames);
            }
            assert_eq!(transferred[1], 256);
            eprintln!(
                "{native_rate}->48000 Hz, retained PCM for 32/256-frame callbacks: {transferred:?}"
            );
        }
    }

    #[test]
    fn disabled_pcm_does_not_poison_sync_or_replace_the_hardware_clock() {
        let (device, context) = synthetic_output(2, 48_000, 48_000);
        let devices = DeviceSet(
            BTreeMap::from([("output".into(), device.clone())]),
            BTreeMap::new(),
        );
        let owner = devices.hardware_clock_progress().unwrap().owner_id;
        assert!(!devices.is_binding_inactive("output", false));
        device.set_active(false);
        let mut bridge = DeviceBridge::new(&context, 48_000).unwrap();
        assert!(!bridge.synchronize_activity(&context));
        assert!(devices.is_binding_inactive("output", false));
        assert_eq!(devices.hardware_clock_progress().unwrap().owner_id, owner);
        device.set_active(true);
        assert!(!devices.is_binding_inactive("output", false));
        assert_eq!(devices.clock_synchronized("output", false), Some(false));
        assert_eq!(devices.hardware_clock_progress().unwrap().owner_id, owner);
    }
    pub(super) fn synthetic_output(
        channels: u16,
        graph_rate: u32,
        native_rate: u32,
    ) -> (Arc<Device>, WorkerContext) {
        synthetic_device(channels, graph_rate, native_rate, false)
    }

    #[test]
    fn first_input_epoch_is_priming_but_later_shortages_remain_active() {
        use mixrelay_audio_io::process::InputActivity;
        let (device, context) = synthetic_device(1, 48_000, 48_000, true);
        let mut pcm = [9.0_f32; 32];
        // SAFETY: unpublished synthetic owner; the fixture serializes both ring
        // roles with full disjoint PCM spans and no native callback or worker.
        unsafe {
            assert_eq!(
                ffi::mr_device_io_bridge_write(
                    device.native.pointer.as_ptr(),
                    [0.25; 32].as_ptr(),
                    32,
                ),
                32
            );
        }
        let allocation = allocation_counter::measure(|| {
            let first = device.read(&mut pcm, 0);
            assert_eq!(first.activity, InputActivity::Priming);
            assert_eq!(first.frames, 0);
            assert!(!first.discontinuity);
            assert!(pcm.iter().all(|sample| *sample == 0.0));
            let stalled = device.read(&mut pcm, 0);
            assert_eq!(stalled.activity, InputActivity::Active);
            assert_eq!(stalled.frames, 0);
        });
        assert_eq!(allocation.count_total, 0);
        // SAFETY: same sole synthetic producer after the reader's freshness reset.
        unsafe {
            assert_eq!(
                ffi::mr_device_io_bridge_write(
                    device.native.pointer.as_ptr(),
                    [0.75; 32].as_ptr(),
                    32,
                ),
                32
            );
        }
        assert_eq!(device.read(&mut pcm, 0), TransferOutcome::complete(32));
        assert_eq!(pcm.map(f32::to_bits), [0.75_f32.to_bits(); 32]);
        context.epoch.fetch_add(1, Ordering::Release);
        let restarted = device.read(&mut pcm, 0);
        assert_eq!(restarted.activity, InputActivity::Active);
        assert_eq!(restarted.frames, 0);
    }

    pub(super) fn synthetic_device(
        channels: u16,
        graph_rate: u32,
        native_rate: u32,
        input: bool,
    ) -> (Arc<Device>, WorkerContext) {
        let native = Arc::new(
            Native::create(
                &Spec {
                    selector: DeviceSelector::PersistentUid {
                        uid: "synthetic-no-device".into(),
                    },
                    input,
                    channels,
                },
                native_rate,
                None,
                0,
                simulated::SimulatedHost::new(),
            )
            .unwrap(),
        );
        // SAFETY: this fresh synthetic owner never connects and no callback/queue
        // role is published yet. Fixtures supply the matching interleaved Float32 ABL themselves.
        assert!(unsafe {
            mixrelay_macos_transport::device_io::test_observation::admit_float_callback(
                native.pointer.as_ptr().cast(),
            )
        });
        let context = WorkerContext {
            activity: Arc::clone(&native.render.activity),
            native: native.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(AtomicBool::new(false)),
            epoch: Arc::new(AtomicU64::new(1)),
            graph_rate: Arc::new(AtomicU32::new(graph_rate)),
            graph_quantum: Arc::new(AtomicU32::new(32)),
            ready_rate: Arc::new(AtomicU32::new(graph_rate)),
            native_rate: Arc::new(AtomicU32::new(native_rate)),
            clock_identity: Arc::new(AtomicU32::new(0)),
            bridge_ready: Arc::new(AtomicBool::new(false)),
            pending_bridge: Arc::new(Mutex::new(None)),
            clock_aligned: Arc::new(AtomicBool::new(false)),
            channels: usize::from(channels),
            input,
        };
        let device = Arc::new(Device {
            input_continuity: device::InputContinuity::default(),
            activity: Arc::clone(&context.activity),
            identity: NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
            native,
            channels: context.channels,
            input,
            stop: context.stop.clone(),
            failed: context.failed.clone(),
            epoch: context.epoch.clone(),
            graph_rate: context.graph_rate.clone(),
            graph_quantum: context.graph_quantum.clone(),
            ready_rate: context.ready_rate.clone(),
            native_rate: context.native_rate.clone(),
            clock_identity: context.clock_identity.clone(),
            bridge_ready: context.bridge_ready.clone(),
            pending_bridge: context.pending_bridge.clone(),
            clock_aligned: context.clock_aligned.clone(),
            worker: None,
        });
        (device, context)
    }
    #[test]
    fn worker_bridge_has_no_priming_output_and_uses_actual_native_rate() {
        for channels in [1, 2, 16, 64] {
            let mut counts = Vec::new();
            for native_rate in [44_100, 96_000] {
                let (device, context) = synthetic_output(channels, 48_000, native_rate);
                let mut bridge = DeviceBridge::new(&context, native_rate).unwrap();
                bridge.pump(&context).unwrap();
                assert_eq!(
                    unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), false) },
                    0
                );
                let source = vec![0.25; 512 * usize::from(channels)];
                assert_eq!(device.write(&source, 0).frames, 512);
                for _ in 0..4 {
                    bridge.pump(&context).unwrap();
                }
                let transferred =
                    unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), false) };
                assert!(transferred > 0);
                assert!(device.bridge_ready.load(Ordering::Acquire));
                assert_eq!(
                    unsafe { mr_device_io_graph_frames(device.native.pointer.as_ptr()) },
                    512
                );
                counts.push(transferred);
            }
            assert!(
                counts[1] > counts[0] * 3 / 2,
                "48k to 96k must produce more native frames than 44.1k: {counts:?}"
            );
        }
    }
    /// A coordinator over a simulated `CoreAudio` publishing one stereo device, with `device`
    /// cached as the owner of the returned graph's output. Graph and cache select the
    /// published device by the same persistent UID, so preparation reuses `device`.
    fn format_fixture(device: &Arc<Device>) -> (MacDevices, GraphDocument) {
        // This unpublished owner represents a graph with only an output, so
        // its cached registration intent must match normal graph admission.
        device.native.output_only.store(true, Ordering::Release);
        let host = simulated::SimulatedHost::new();
        let selector = DeviceSelector::PersistentUid {
            uid: host.plug("io.mixrelay.test.format", 2, 48_000, 0.0),
        };
        let status = Arc::new(AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        let resources = MacDevices::with_host(status, 48_000, 256, host);
        resources.cache.lock().unwrap().insert(
            "output".into(),
            (
                Spec {
                    selector: selector.clone(),
                    input: false,
                    channels: 2,
                },
                Arc::downgrade(device),
            ),
        );
        let graph = GraphDocument::new(
            1,
            vec![mixrelay_audio_graph::NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: "output".into(),
                label: None,
                config: NodeConfig::AudioDeviceOutput(
                    mixrelay_audio_graph::AudioDeviceOutputConfig {
                        selector,
                        input_channels: 2,
                    },
                ),
            }],
            vec![],
        );
        (resources, graph)
    }

    #[test]
    fn format_reservation_cancellation_and_reclaim_are_safe() {
        let (device, _context) = synthetic_output(2, 48_000, 44_100);
        let (resources, graph) = format_fixture(&device);
        let old = RenderFormat {
            sample_rate: 48_000,
            block_frames: 256,
        };
        let new = RenderFormat {
            sample_rate: 96_000,
            block_frames: 256,
        };
        assert_eq!(device.write(&[0.25; 512], 0).frames, 256);
        let cancelled = resources
            .prepare_format_change(
                &graph,
                mixrelay_engine::PlaybackBindings::Preserve,
                old,
                new,
            )
            .unwrap();
        assert!(!resources.handoff.idle());
        assert!(
            resources
                .prepare_format_change(
                    &graph,
                    mixrelay_engine::PlaybackBindings::Preserve,
                    old,
                    new
                )
                .is_err()
        );
        resources.reclaim();
        assert!(
            !resources.handoff.idle(),
            "recovery must not take a format reservation"
        );
        assert_eq!(device.graph_rate.load(Ordering::Acquire), 48_000);
        assert_eq!(
            unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), true) },
            256
        );
        drop(cancelled);
        assert!(
            resources.handoff.idle(),
            "cancellation releases resource preparation"
        );
        assert!(device.pending_bridge.lock().unwrap().is_none());
        resources
            .prepare_format_change(
                &graph,
                mixrelay_engine::PlaybackBindings::Preserve,
                old,
                new,
            )
            .unwrap()
            .commit();
        assert_eq!(device.graph_rate.load(Ordering::Acquire), 96_000);
    }

    #[test]
    fn committed_format_rejects_mismatched_pcm_and_activates_pending_slot() {
        let (device, context) = synthetic_output(2, 48_000, 44_100);
        let (resources, graph) = format_fixture(&device);
        let old = RenderFormat {
            sample_rate: 48_000,
            block_frames: 256,
        };
        let new = RenderFormat {
            sample_rate: 96_000,
            block_frames: 256,
        };
        assert_eq!(device.write(&[0.25; 512], 0).frames, 256);
        resources
            .prepare_format_change(
                &graph,
                mixrelay_engine::PlaybackBindings::Preserve,
                old,
                new,
            )
            .unwrap()
            .commit();
        assert!(
            !resources.handoff.idle(),
            "commit retains the graph reservation until activation"
        );
        assert_eq!(
            (
                device.graph_rate.load(Ordering::Acquire),
                device.native_rate.load(Ordering::Acquire),
            ),
            (96_000, 44_100),
            "publishing the processing rate must preserve the physical device rate"
        );
        assert_eq!(
            device.write(&[0.25; 512], 0).frames,
            0,
            "old converter must not accept new-rate PCM"
        );
        assert!(
            resources
                .prepare_format_change(
                    &graph,
                    mixrelay_engine::PlaybackBindings::Preserve,
                    new,
                    old
                )
                .is_err(),
            "pending publication slot cannot be replaced"
        );
        let mut prepared = device.pending_bridge.lock().unwrap().take().unwrap();
        prepared.activate(&context);
        assert_eq!(
            unsafe { mr_device_io_queued(device.native.pointer.as_ptr(), true) },
            0
        );
        assert_eq!(
            (prepared.graph_rate, prepared.native_rate),
            (96_000, 44_100)
        );
    }
    #[test]
    fn same_native_device_bypasses_src_but_unknown_and_other_clocks_do_not() {
        let (master, _) = synthetic_output(2, 48_000, 48_000);
        let (peer, peer_context) = synthetic_output(2, 48_000, 48_000);
        let (other, other_context) = synthetic_output(2, 48_000, 48_000);
        master.clock_identity.store(41, Ordering::Release);
        peer.clock_identity.store(41, Ordering::Release);
        other.clock_identity.store(42, Ordering::Release);
        let devices = DeviceSet(
            BTreeMap::from([
                ("master".into(), master.clone()),
                ("peer".into(), peer.clone()),
                ("other".into(), other),
            ]),
            BTreeMap::new(),
        );
        // Reproduce the previous blanket-independent policy under exactly the
        // same channel/rate/input conditions as the new relationship decision.
        let mut before = DeviceBridge::new(&peer_context, 48_000).unwrap();
        let source = [0.25; 1024];
        let mut output = [0.0; 512];
        before.bridge.push_interleaved(&source[..512]).unwrap();
        before.bridge.push_interleaved(&source[..512]).unwrap();
        let previous = before.bridge.pull_interleaved(&mut output).unwrap();
        devices.set_clock_master(Some(master.identity));
        assert!(peer_context.clock_aligned.load(Ordering::Acquire));
        assert!(!other_context.clock_aligned.load(Ordering::Acquire));
        let mut after = DeviceBridge::new(&peer_context, 48_000).unwrap();
        after.bridge.push_interleaved(&source[..512]).unwrap();
        after.bridge.push_interleaved(&source[..512]).unwrap();
        let current = after.bridge.pull_interleaved(&mut output).unwrap();
        assert!(previous.algorithmic_delay_frames > 0);
        assert_eq!(current.algorithmic_delay_frames, 0);
        assert_eq!(current.transferred_frames, 256);
        assert!(
            output
                .iter()
                .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
        );
        eprintln!(
            "same-device 2ch/48000Hz/256-frame pull: SRC delay {} -> {} frames, bypass PCM bit-exact",
            previous.algorithmic_delay_frames, current.algorithmic_delay_frames
        );

        // Zero means unverified (including user aggregates), not shared clock.
        master.clock_identity.store(0, Ordering::Release);
        peer.clock_identity.store(0, Ordering::Release);
        devices.set_clock_master(Some(master.identity));
        assert!(!peer_context.clock_aligned.load(Ordering::Acquire));
        master.clock_identity.store(41, Ordering::Release);
        peer.clock_identity.store(41, Ordering::Release);
        peer.native_rate.store(44_100, Ordering::Release);
        devices.set_clock_master(Some(master.identity));
        assert!(!peer_context.clock_aligned.load(Ordering::Acquire));
        peer.native_rate.store(48_000, Ordering::Release);
        master.failed.store(true, Ordering::Release);
        devices.set_clock_master(Some(master.identity));
        assert!(!peer_context.clock_aligned.load(Ordering::Acquire));
        devices.set_clock_master(None);
        assert!(!master.clock_aligned.load(Ordering::Acquire));
    }

    #[test]
    fn hardware_master_identity_survives_bridge_priming_and_epoch_changes() {
        let (device, context) = synthetic_output(2, 48_000, 48_000);
        let devices = DeviceSet(
            BTreeMap::from([("output".into(), device.clone())]),
            BTreeMap::new(),
        );
        let owner = devices.hardware_clock_progress().unwrap().owner_id;
        devices.set_clock_master(Some(owner));
        assert!(context.clock_aligned.load(Ordering::Acquire));
        device.ready_rate.store(0, Ordering::Release);
        device.epoch.fetch_add(1, Ordering::Release);
        assert_eq!(devices.hardware_clock_progress().unwrap().owner_id, owner);
        devices.set_clock_master(Some(owner));
        assert!(context.clock_aligned.load(Ordering::Acquire));
        let mut bridge = DeviceBridge::new(&context, 48_000).unwrap();
        assert!(bridge.clock_aligned);
        device.ready_rate.store(48_000, Ordering::Release);
        let source = [0.25; 1024];
        assert_eq!(device.write(&source, 0).frames, 512);
        for _ in 0..4 {
            bridge.pump(&context).unwrap();
        }
        bridge.observe(&context, true).unwrap();
        assert_eq!(
            bridge.bridge.correction_ppm().to_bits(),
            0.0_f64.to_bits(),
            "native master cannot fight its own pacing PI"
        );
        devices.set_clock_master(None);
        assert!(!context.clock_aligned.load(Ordering::Acquire));
        device.failed.store(true, Ordering::Release);
        assert!(devices.hardware_clock_progress().is_none());
    }
    #[test]
    fn prepared_bridge_and_graph_rate_publish_as_one_worker_observation() {
        let (device, _) = synthetic_output(2, 48_000, 44_100);
        let other = device.clone();
        let worker = thread::spawn(move || {
            loop {
                let mut pending = other.pending_bridge.lock().unwrap();
                if let Some(bridge) = pending.take() {
                    assert_eq!(other.graph_rate.load(Ordering::Acquire), bridge.graph_rate);
                    return;
                }
                drop(pending);
                thread::yield_now();
            }
        });
        Box::new(PreparedDeviceFormat {
            reservation: ResourceHandoff::<DeviceSet>::default()
                .reserve_format()
                .unwrap(),
            devices: vec![(
                device,
                DeviceBridge::prepare(2, false, 44_100, 96_000, false).unwrap(),
            )],
            published: Arc::new(PublishedFormat::new(48_000, 256)),
            requested_rate: 96_000,
            requested_buffer: 256,
            previous_buffer: 256,
        })
        .commit();
        worker.join().unwrap();
    }
    #[test]
    fn wrong_direction_and_invalid_width_fail_before_callbacks() {
        for spec in [
            Spec {
                selector: DeviceSelector::DefaultOutput,
                input: true,
                channels: 2,
            },
            Spec {
                selector: DeviceSelector::DefaultInput,
                input: false,
                channels: 2,
            },
            Spec {
                selector: DeviceSelector::DefaultInput,
                input: true,
                channels: 65,
            },
        ] {
            assert!(Device::open(&spec, 48_000, simulated::SimulatedHost::new()).is_err());
        }
    }
    #[test]
    fn empty_graph_needs_no_native_devices() {
        let document = GraphDocument::new(1, vec![], vec![]);
        assert!(specs(&document).is_empty());
    }
}

#[cfg(any(test, feature = "native-lab"))]
pub(crate) mod pcm_probe;

#[cfg(test)]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "native device transition stalled"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Owned `CoreAudio` substitutes for application-level controller contract tests.
#[cfg(any(test, feature = "test-util"))]
pub(crate) mod test_support {
    pub use super::controller_fixture::{
        Activation, ActiveDevices, OwnerLifetimes, activation, assert_cached_selection,
        cache_is_empty, has_pending,
    };
    pub use super::simulated::{CAPTURED, DEFAULT_BUFFER, SimulatedHost};
    /// Shares one simulated HAL with the graph resource owner.
    #[must_use]
    pub fn devices(
        status: std::sync::Arc<mixrelay_engine::session::AdapterRuntimeStatus>,
        rate: u32,
        frames: u32,
        host: std::sync::Arc<SimulatedHost>,
    ) -> super::MacDevices {
        super::MacDevices::with_host(status, rate, frames, host)
    }
}
