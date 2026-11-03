use mixrelay_driver_adapters::DriverClient;
use mixrelay_driver_control::{DriverCommand, DriverError, DriverSnapshot};
use tauri::Manager as _;

/// Read the installed Driver owner.
#[tauri::command]
pub(crate) async fn driver_status() -> Result<DriverSnapshot, String> {
    let client = DriverClient::session().map_err(|error| error.to_string())?;
    let snapshot = match client.snapshot().await {
        Ok(snapshot) => Ok(snapshot),
        Err(DriverError::NotReachable(_)) => {
            tauri::async_runtime::spawn_blocking(unavailable_snapshot)
                .await
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }?;
    Ok(snapshot)
}

/// Apply Driver device settings.
#[tauri::command]
pub(crate) async fn driver_mutate(
    app: tauri::AppHandle,
    command: DriverCommand,
) -> Result<DriverSnapshot, String> {
    let client = DriverClient::session().map_err(|error| error.to_string())?;
    let snapshot = client
        .mutate(&command)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
        menu.observe_snapshot(&snapshot);
    }
    Ok(snapshot)
}

fn unavailable_snapshot() -> DriverSnapshot {
    let installed = installed_driver_path().is_file();
    absent_owner_snapshot(installed, installed_driver_version())
}

use mixrelay_desktop_application::devices::absent_owner_snapshot;

#[cfg(target_os = "macos")]
fn installed_driver_path() -> &'static std::path::Path {
    std::path::Path::new("/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver/Contents/Info.plist")
}

#[cfg(target_os = "linux")]
fn installed_driver_path() -> &'static std::path::Path {
    std::path::Path::new("/usr/bin/mixrelay-driver-service")
}

/// Reads the installed Driver artifact's version, never the graph application's version.
#[cfg(target_os = "macos")]
pub(crate) fn installed_driver_version() -> Option<String> {
    installed_plist_version(installed_driver_path())
}

#[cfg(target_os = "macos")]
fn installed_plist_version(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    installed_version_text(&String::from_utf8(output.stdout).ok()?)
}

/// Reads the installed app bundle's version.
#[cfg(target_os = "macos")]
pub(crate) fn installed_mixrelay_version() -> Option<String> {
    installed_plist_version(std::path::Path::new(
        mixrelay_macos_transport::install_contract::APP_INFO_PATH,
    ))
}

/// Reads the package actually owning the graph app executable, without starting that app.
#[cfg(target_os = "linux")]
pub(crate) fn installed_mixrelay_version() -> Option<String> {
    const EXECUTABLE: &str = "/usr/bin/mixrelay-desktop";
    if !std::path::Path::new(EXECUTABLE).is_file() {
        return None;
    }
    let deb_version = std::process::Command::new("dpkg-query")
        .args(["--search", EXECUTABLE])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|owners| {
            let owner = owners
                .lines()
                .find_map(|line| line.strip_suffix(": /usr/bin/mixrelay-desktop"))?;
            package_version("dpkg-query", &["--show", "--showformat=${Version}", owner])
        });
    deb_version.or_else(|| {
        package_version(
            "rpm",
            &[
                "--query",
                "--file",
                EXECUTABLE,
                "--queryformat",
                "%{VERSION}",
            ],
        )
    })
}

#[cfg(target_os = "linux")]
fn package_version(program: &str, arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    installed_version_text(&String::from_utf8(output.stdout).ok()?)
}

/// Reads the receipt shipped with the installed Linux Driver package.
#[cfg(target_os = "linux")]
pub(crate) fn installed_driver_version() -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Receipt {
        version: String,
    }
    if !installed_driver_path().is_file() {
        return None;
    }
    let bytes = std::fs::read("/usr/share/mixrelay-driver/version.json").ok()?;
    let receipt: Receipt = serde_json::from_slice(&bytes).ok()?;
    installed_version_text(&receipt.version)
}

fn installed_version_text(value: &str) -> Option<String> {
    let value = value.trim();
    mixrelay_desktop_application::updates::stable_version(value)
        .ok()
        .map(|_| value.to_owned())
}

pub(crate) async fn stop_driver() -> Result<DriverSnapshot, String> {
    let result = mixrelay_driver_adapters::DriverClient::session()
        .map_err(|error| error.to_string())?
        .set_active(false)
        .await;
    if matches!(
        &result,
        Err(mixrelay_driver_control::DriverError::NotReachable(_))
    ) && let Ok(snapshot) = crate::driver_control::driver_status().await
        && snapshot.state == mixrelay_driver_control::DriverState::NotInstalled
        && snapshot.settings.devices.is_empty()
    {
        // An uninstalled Driver has nothing to stop. A merely unloaded or
        // unreachable installed backend is not proof that native audio stopped.
        return Ok(snapshot);
    }
    result.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_driver_control::DriverState;

    #[test]
    fn absent_service_never_reports_ready_or_mutation_capabilities() {
        for installed in [true, false] {
            let snapshot = absent_owner_snapshot(installed, None);
            assert_ne!(snapshot.state, DriverState::Ready);
            assert!(!snapshot.settings.can_create);
            assert!(!snapshot.settings.can_rename);
            assert!(snapshot.settings.creation_template.is_none());
            assert_eq!(snapshot.installed_version, None);
            assert!(snapshot.persistence_error.is_none());
        }
    }

    #[test]
    fn installed_artifact_version_is_preserved_without_a_running_service() {
        let snapshot = absent_owner_snapshot(true, Some("0.2.3".into()));
        assert_eq!(snapshot.state, DriverState::InstalledNotLoaded);
        assert_eq!(snapshot.installed_version.as_deref(), Some("0.2.3"));
    }

    #[test]
    fn missing_or_invalid_versions_are_not_guessed() {
        assert_eq!(installed_version_text(" 0.2.3\n"), Some("0.2.3".into()));
        for value in ["", "unknown", "0.2", "0.2.3-dev", "0.2.3+local"] {
            assert_eq!(installed_version_text(value), None);
        }
    }
}
