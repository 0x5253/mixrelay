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
#[cfg(test)]
mod device_fixture;
mod device_set;
pub(super) mod direct;
mod ffi;
mod format;
mod graph_resources;
mod group_plan;
mod handoff;
mod host;
pub(super) mod latency;
mod native;
#[cfg(any(test, feature = "native-lab"))]
pub(crate) mod pcm_probe;
mod permission;
mod recovery;
mod retirement;
mod routing_clock;
mod runtime_status;
#[cfg(any(test, feature = "test-util"))]
mod simulated;
mod spec;
mod telemetry;
mod worker;
mod worker_retirement;

use aggregate::Aggregate;
use device::Device;
#[cfg(test)]
use device_fixture::wait_until;
pub(crate) use device_set::DeviceSet;
use format::PublishedFormat;
use handoff::{Publication, ResourceConsumer, ResourceHandoff};
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use spec::Spec;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

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
        status: std::sync::Arc<mixrelay_engine_core::session::AdapterRuntimeStatus>,
        rate: u32,
        frames: u32,
        host: std::sync::Arc<SimulatedHost>,
    ) -> super::MacDevices {
        super::MacDevices::with_host(status, rate, frames, host)
    }
}
