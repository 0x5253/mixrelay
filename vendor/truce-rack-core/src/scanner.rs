//! Plugin discovery trait.
//!
//! Each format wrapper exposes a `*Scanner` type that
//! implements [`PluginScanner`]. Hosts may also implement
//! their own composite scanners that aggregate results across
//! formats (`MultiScanner { clap, vst3, au }` etc.) — there's
//! nothing format-specific in the trait.

use crate::error::Result;
use crate::info::PluginInfo;
use std::path::Path;

/// Discover and load audio plugins of a single format.
///
/// # Thread safety
///
/// Scans are **off the audio thread**. They walk the filesystem,
/// open dylibs, and can take seconds (a fresh AU scan touches
/// 100+ plugins on a typical Mac). Hosts should never call
/// `scan` from a real-time context — wrap in a worker thread
/// if you need a non-blocking discovery flow.
pub trait PluginScanner {
    /// Concrete plugin type this scanner produces. Always
    /// implements [`crate::Plugin`] for at least one sample
    /// precision.
    type Plugin;

    /// Scan default OS plugin directories for this format.
    /// Each format wrapper picks the conventional paths
    /// (`~/Library/Audio/Plug-Ins/CLAP` and `/Library/...`
    /// for CLAP on macOS, the registry on Windows, etc.).
    ///
    /// # Errors
    /// Directory enumeration and candidate-loading failures propagate. An incomplete scan
    /// must not be presented as an authoritative empty or partial catalog: the caller uses
    /// errors to retain its last successful snapshot. Absent optional default directories
    /// may be skipped, but permission and malformed-candidate errors are not empty success.
    fn scan(&self) -> Result<Vec<PluginInfo>>;

    /// Scan a specific directory or format-supported bundle. Useful for hosts that bundle
    /// their own plugins or that want to test against a known fixture without searching
    /// unrelated default directories.
    ///
    /// # Errors
    /// Same as [`PluginScanner::scan`]. An explicit missing path is an error; only optional
    /// default locations have missing-directory semantics. Registry-based formats may not
    /// support filesystem-scoped enumeration and must document that limitation.
    fn scan_path(&self, path: &Path) -> Result<Vec<PluginInfo>>;

    /// Materialise an instance from the [`PluginInfo`] returned
    /// by `scan` / `scan_path`. Most wrappers actually dlopen
    /// the plugin's dylib at this point; expect file I/O.
    ///
    /// # Errors
    /// [`crate::Error::PluginNotFound`] when `info.unique_id`
    /// doesn't match anything in this scanner's index;
    /// [`crate::Error::LoadFailed`] on dylib / signature
    /// errors.
    fn load(&self, info: &PluginInfo) -> Result<Self::Plugin>;
}
