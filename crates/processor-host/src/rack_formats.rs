//! Plug-in format hosting built on the shared `truce-rack` host contract.
//!
//! Discovery and probing run only in disposable child processes. A third-party binary can
//! therefore crash or hang without taking down the graph/control process. DSP instances run in
//! the same bounded shared-memory worker model for every supported format.
//!
//! [`adapters`] is the only module that names a format crate. Layout negotiation, the scan and
//! probe children and the worker instance work against the format-neutral `truce-rack` traits.

use serde::Serialize;

pub(crate) mod adapters;
mod effect_layout;
pub(crate) mod identity;
pub(crate) mod layouts;
pub(crate) mod probe;
pub(crate) mod scan;
pub(crate) mod worker;

#[cfg(test)]
#[cfg(target_os = "macos")]
mod apple_system_tests;
#[cfg(test)]
mod fixture;

pub const SCAN_ARGUMENT: &str = "--mixrelay-plugin-scan";
pub const PROBE_ARGUMENT: &str = "--mixrelay-plugin-probe";
pub const WORKER_ARGUMENT: &str = "--mixrelay-plugin-worker";

/// Watches the parent and prepares this scan or probe child before any vendor code runs.
fn enter_child_process() -> Result<std::thread::JoinHandle<()>, String> {
    let parent = crate::dsp_lifecycle::watch_parent(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        crate::dsp_lifecycle::PARENT_GRACE,
    )
    .map_err(|error| error.to_string())?;
    crate::worker::prepare_plugin_process()?;
    Ok(parent)
}

/// Writes a child's single JSON result line to stdout for the parent to capture.
fn print_json(value: &impl Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| error.to_string())?
    );
    Ok(())
}
