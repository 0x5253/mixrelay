//! CLAP format adapter for bundle discovery and hosted plugin instances.
//!
//! Discovery initializes a bundle entry, reads factory descriptors, then deinitializes it.
//! Loading retains the entry and dynamic library for one owned plugin instance. Teardown closes
//! the editor, stops processing, deactivates and destroys the plugin, then deinitializes its entry.
//! Native callbacks retain access to the host context throughout that ordered teardown.

mod audio_ports;
#[cfg(test)]
mod boundary_tests;
mod core;
mod editor;
mod events;
mod host;
mod library;
mod lifecycle;
mod parameter_ids;
mod parameters;
mod process;
mod scanner;
#[cfg(test)]
mod scanner_tests;
mod state;
#[cfg(test)]
mod tests;
mod timer_support;

use audio_ports::AudioPortCatalog;
use clap_sys::ext::{
    latency::clap_plugin_latency, state::clap_plugin_state,
    timer_support::clap_plugin_timer_support,
};
use clap_sys::plugin::clap_plugin;
use editor::EditorState;
use host::HostContext;
use library::LoadedLibrary;
use lifecycle::ProcessingState;
use parameters::Parameters;
use truce_rack_core::info::PluginInfo;

pub use scanner::{ClapScanner, default_clap_paths};

/// Format identifier — used as the `format` field on
/// [`PluginInfo`] entries this crate returns.
pub const FORMAT: &str = "clap";

/// Filename extension every CLAP plugin uses, including the
/// leading dot.
pub const CLAP_EXTENSION: &str = ".clap";

/// Symbol name `clap_entry` plugins must export.
const ENTRY_SYMBOL: &[u8] = b"clap_entry\0";

/// One loaded CLAP plugin instance.
///
/// Holds the raw `*const clap_plugin` pointer plus the metadata
/// needed for the truce-rack-core trait impls. The pointer is owned —
/// `Drop` calls `clap_plugin::destroy` and `clap_entry::deinit`.
pub struct ClapPlugin {
    info: PluginInfo,
    audio_port_catalog: AudioPortCatalog,
    plugin: *const clap_plugin,
    /// Stable host context outlives plugin destroy and entry deinit in `Drop`.
    host_context: Box<HostContext>,
    library: LoadedLibrary,
    processing: ProcessingState,
    parameters: Parameters,
    editor: EditorState,
    state_ext: *const clap_plugin_state,
    timer_ext: *const clap_plugin_timer_support,
    latency_ext: *const clap_plugin_latency,
}

// SAFETY: Moving the wrapper does not transfer or destroy native ownership. The host serializes
// access through its instance mutex; lifecycle and host-service calls additionally verify the
// captured creator thread, while DSP calls use the adapter's symbolic audio-thread scope.
unsafe impl Send for ClapPlugin {}
