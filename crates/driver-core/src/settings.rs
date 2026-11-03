//! Retained Driver configuration and its durable storage boundary.

use crate::{DriverDeviceConfig, DriverError};

/// Saved intent, validated by the manager before any native initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSettings {
    /// Revision retained across service restarts.
    pub revision: u64,
    /// Whether the retained devices should be published.
    pub active: bool,
    /// Desired devices, including those retained while the Driver is stopped.
    pub devices: Vec<DriverDeviceConfig>,
}

/// Loads and commits one complete retained configuration outside the audio callback.
pub trait SettingsStore: Send {
    /// Reads the saved configuration; `None` means no configuration exists.
    /// An explicitly saved empty inventory is still `Some`.
    ///
    /// # Errors
    /// Returns read or decoding failures without replacing the saved configuration.
    fn load(&self) -> Result<Option<StoredSettings>, DriverError>;

    /// Atomically replaces the saved configuration.
    ///
    /// # Errors
    /// Returns a storage failure while preserving the previously committed configuration.
    fn save(&self, settings: &StoredSettings) -> Result<(), DriverError>;
}
