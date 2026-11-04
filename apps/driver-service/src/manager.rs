//! Composes the native Driver backend with persisted settings and the product clock.

use std::path::PathBuf;

use anyhow::{Context, Result};
use mixrelay_driver_adapters::{FileSettingsStore, SystemLatencyGenerations};
use mixrelay_driver_core::{DriverManager, DriverProcessId, DriverRuntime};

pub(crate) fn open_manager(path: PathBuf) -> Result<DriverManager> {
    let mut manager = DriverManager::open(
        open_backend()?,
        FileSettingsStore::new(path),
        driver_runtime()?,
    )
    .context("restore Driver settings")?;
    #[cfg(target_os = "macos")]
    let installed_version = installed_driver_version();
    // The running service binary carries the Driver release, independently of the graph app.
    #[cfg(target_os = "linux")]
    let installed_version = Some(env!("CARGO_PKG_VERSION").into());
    manager.set_installed_version(installed_version);
    Ok(manager)
}

#[cfg(target_os = "macos")]
fn open_backend() -> Result<mixrelay_macos_driver::MacDriver> {
    let policy = transport_compatibility(std::env::var("MIXRELAY_TRANSPORT_COMPATIBILITY"))?;
    let policy = mixrelay_macos_transport::control_inventory::ProducerPolicy::new(&policy)
        .context("invalid transport compatibility policy")?;
    mixrelay_macos_driver::MacDriver::open(policy).context("open macOS Driver")
}

#[cfg(target_os = "linux")]
fn open_backend() -> Result<mixrelay_linux_driver::LinuxDriver> {
    mixrelay_linux_driver::LinuxDriver::open().context("open PipeWire Driver")
}

fn driver_runtime() -> Result<DriverRuntime> {
    Ok(DriverRuntime::new(
        DriverProcessId::new(std::process::id())?,
        Box::new(mixrelay_time::StdClock::new()),
        Box::new(SystemLatencyGenerations),
    ))
}

#[cfg(any(target_os = "macos", test))]
fn transport_compatibility(value: Result<String, std::env::VarError>) -> Result<Vec<String>> {
    // Transport compatibility is configured in the service, outside HAL binaries.
    let value = match value {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) =>
            "com.tencent.tencentmeeting,net.whatsapp.WhatsApp,com.motu.DigitalPerformer,com.vmware.fusion,com.goodnotesapp.x".into(),
        Err(error) => return Err(error).context("read transport compatibility override"),
    };
    mixrelay_driver_core::parse_transport_compatibility(&value)
        .context("parse MIXRELAY_TRANSPORT_COMPATIBILITY")
}

#[cfg(target_os = "macos")]
fn installed_driver_version() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args([
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "-o",
            "-",
            "/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver/Contents/Info.plist",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!version.is_empty()).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn transport_policy_defaults_and_development_override() {
        assert_eq!(
            transport_compatibility(Err(std::env::VarError::NotPresent))
                .unwrap()
                .len(),
            5
        );
        assert!(
            transport_compatibility(Ok(String::new()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            transport_compatibility(Ok("com.test.App".into())).unwrap(),
            ["com.test.App"]
        );
        assert!(transport_compatibility(Ok("com.*".into())).is_err());
        assert!(
            transport_compatibility(Err(std::env::VarError::NotUnicode(OsString::from("bad"))))
                .is_err()
        );
    }
}
