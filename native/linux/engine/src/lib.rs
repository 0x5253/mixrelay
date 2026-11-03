//! `PipeWire` engine resource ownership, native callbacks and recovery adapters.
#![cfg(target_os = "linux")]
mod linux;
mod processing_failure;
pub use linux::{
    DeviceRecoveryWorker, EndpointDriver, EndpointObservation, FormatMonitor,
    LinuxVirtualEndpointBackend, WorkerError, adapter_profile, create_initial_processor,
    prepare_runtime, run_audio_worker, spawn_reclaimers, start_recovery,
};
