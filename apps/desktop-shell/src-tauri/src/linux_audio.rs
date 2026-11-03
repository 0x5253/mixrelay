//! Native Linux inventory adapters for desktop commands.
use mixrelay_audio_io::{AudioDeviceDescriptor, CapturableApplication};
/// The controller owns the executable metadata cache across inventory refreshes.
pub(crate) type ApplicationDiscovery =
    mixrelay_linux_audio_io::ApplicationDiscovery<mixrelay_time::StdClock>;

/// Device application attribution is not available from the Linux backend yet.
#[tauri::command]
pub(crate) fn list_device_app_activity()
-> Result<std::collections::BTreeMap<String, Vec<serde_json::Value>>, String> {
    Err("app activity is not available on Linux yet".to_owned())
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) fn list_capturable_applications(
    discovery: tauri::State<'_, std::sync::Arc<ApplicationDiscovery>>,
) -> Result<Vec<CapturableApplication>, String> {
    discovery.applications().map_err(|error| error.to_string())
}

/// Lists current `PipeWire` audio sources and sinks with their negotiated channel capacities.
///
/// # Errors
///
/// Returns a diagnostic when the active user `PipeWire` registry cannot be queried.
#[tauri::command]
pub(crate) fn list_audio_devices() -> Result<Vec<AudioDeviceDescriptor>, String> {
    mixrelay_linux_audio_io::devices().map_err(|error| error.to_string())
}
