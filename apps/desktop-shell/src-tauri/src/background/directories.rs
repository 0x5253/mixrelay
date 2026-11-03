//! Persistent data and private control endpoints follow separate OS directory contracts.
use std::path::PathBuf;
use tauri::Manager as _;

/// The app's persistent data directory (`io.mixrelay.desktop`): settings and journal.
pub(crate) fn data_directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|error| error.to_string())
}

/// Transient control endpoints use the normal user's private runtime directory
/// on Linux. Persistent XDG data ancestors may legitimately be group-writable.
#[cfg(target_os = "linux")]
pub(crate) fn control_directory(_app: &tauri::AppHandle) -> std::io::Result<PathBuf> {
    mixrelay_local_socket::session_socket_directory().map(|directory| directory.join("control"))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn control_directory(app: &tauri::AppHandle) -> std::io::Result<PathBuf> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("control"))
        .map_err(std::io::Error::other)
}
