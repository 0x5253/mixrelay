//! `CoreAudio` engine resource ownership, rendering and endpoint IPC adapters.
#![cfg(target_os = "macos")]

mod endpoint_backend;
pub mod endpoint_service;
mod macos;
mod macos_devices;
mod macos_pacing;
mod maintenance;

pub use endpoint_backend::{EndpointDriver, MacVirtualEndpointBackend};
pub use macos::runtime_latency::{
    LatencyPublication, PublicationError, PublicationFuture, Reporter as RuntimeLatencyReporter,
};
pub use macos::{AudioWorkerError, create_initial_processor, prepare_runtime, start_audio_worker};
pub use macos_devices::MacDevices;
pub use maintenance::{MaintenanceWorker, start_format_monitor, start_reclaimer};

#[cfg(test)]
mod shared_epoch_tests;

/// Explicit installed-engine observations used by the application lab.
#[cfg(feature = "native-lab")]
pub mod native_lab {
    /// Exercises the installed native client after the application reads actual Driver state.
    pub fn installed_driver_pcm_soak(
        snapshot: impl FnMut() -> mixrelay_driver_core::DriverSnapshot,
    ) {
        super::macos_devices::pcm_probe::installed_driver_pcm_soak(snapshot);
    }

    /// Whether this process has already published its sole endpoint-service owner.
    #[must_use]
    pub fn service_initialized() -> bool {
        super::endpoint_service::initialized()
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod test_support {
    pub use crate::macos_devices::test_support::{
        Activation, ActiveDevices, CAPTURED, DEFAULT_BUFFER, OwnerLifetimes, SimulatedHost,
        activation, assert_cached_selection, cache_is_empty, devices, has_pending,
    };
    /// Product capabilities without constructing a native device owner.
    #[must_use]
    pub fn adapter_profile() -> mixrelay_engine_core::session::AdapterProfile {
        crate::macos::adapter_profile()
    }
    /// Starts an owned worker for application observer shutdown-order fixtures.
    /// # Errors
    /// Returns a native thread creation failure before publishing an owner.
    pub fn maintenance_worker(
        name: &'static str,
        work: impl FnOnce(&std::sync::atomic::AtomicBool) + Send + 'static,
    ) -> Result<crate::MaintenanceWorker, String> {
        crate::MaintenanceWorker::start(name, work)
    }
}
