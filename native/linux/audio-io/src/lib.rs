//! Native linux audio discovery adapters.
#[cfg(target_os = "linux")]
mod discovery;
#[cfg(target_os = "linux")]
pub use discovery::{
    ApplicationDiscovery, PipeWireTarget, applications, bounded_pipewire_output, devices,
    resolve_device_target, targets,
};
#[cfg(any(target_os = "linux", test))]
mod metadata_cache;
#[cfg(any(target_os = "linux", test))]
mod metadata_lookup;
#[cfg(any(target_os = "linux", test))]
pub mod process_identity;
