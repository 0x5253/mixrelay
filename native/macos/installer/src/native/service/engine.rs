//! The same Driver transaction registers, stops and restores its on-demand audio engine.
use super::super::{
    command,
    launchctl::{self, ENGINE, RegistrationState},
};
use anyhow::{Result, ensure};

pub(super) fn registered() -> Result<bool> {
    Ok(matches!(
        launchctl::inspect(&ENGINE)?,
        RegistrationState::Loaded { .. }
    ))
}

// Unlike the Driver job, engine bootstrap and bootout accept diagnostic output.
pub(super) fn drain() -> Result<()> {
    if registered()? {
        let output = command::run_command(launchctl::PROGRAM, &["bootout", &ENGINE.target()])?;
        ensure!(output.status.success(), "audio engine bootout failed");
        ensure!(!registered()?, "audio engine remains registered");
    }
    Ok(())
}

pub(super) fn register() -> Result<()> {
    let output =
        command::run_command(launchctl::PROGRAM, &["bootstrap", "system", ENGINE.plist()])?;
    ensure!(output.status.success(), "audio engine registration failed");
    ensure!(
        registered()?,
        "audio engine registration was not acknowledged"
    );
    Ok(())
}
