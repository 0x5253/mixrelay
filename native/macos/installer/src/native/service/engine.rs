//! The same Driver transaction registers, stops and restores its on-demand audio engine.
use super::super::launchctl;
use crate::RUNTIME_LAUNCHD_PATH;
use anyhow::{Result, ensure};
const TARGET: &str = "system/io.mixrelay.audio.engine";

pub(super) fn registered() -> Result<bool> {
    Ok(matches!(
        launchctl::inspect_engine_registration()?,
        launchctl::RegistrationState::Loaded { .. }
    ))
}

pub(super) fn drain() -> Result<()> {
    if registered()? {
        let output = launchctl::run_command("/bin/launchctl", &["bootout", TARGET])?;
        ensure!(output.status.success(), "audio engine bootout failed");
        ensure!(!registered()?, "audio engine remains registered");
    }
    Ok(())
}

pub(super) fn register() -> Result<()> {
    let output = launchctl::run_command(
        "/bin/launchctl",
        &["bootstrap", "system", RUNTIME_LAUNCHD_PATH],
    )?;
    ensure!(output.status.success(), "audio engine registration failed");
    ensure!(
        registered()?,
        "audio engine registration was not acknowledged"
    );
    Ok(())
}
