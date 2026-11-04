//! `PipeWire` engine resource ownership, native callbacks and recovery adapters.
#![cfg(target_os = "linux")]
mod applications;
mod backend;
mod devices;
mod endpoints;
mod format;
mod processing;
mod runtime;
mod source_mute;
#[cfg(test)]
mod test_support;

pub use backend::{DeviceRecoveryWorker, LinuxVirtualEndpointBackend, start_recovery};
pub use format::{EndpointObservation, FormatMonitor};
pub use processing::{WorkerError, run_audio_worker};
pub use runtime::{
    EndpointDriver, adapter_profile, create_initial_processor, prepare_runtime, spawn_reclaimers,
};
