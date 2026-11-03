//! Native macos audio discovery adapters.
#[cfg(target_os = "macos")]
mod discovery;
#[cfg(target_os = "macos")]
pub use discovery::{devices, selected_devices};
