use std::{error::Error, process::Command};

/// Inherit compiler output so failed native or Rust builds cannot be masked by
/// a later packaging command. No command string is passed through a shell.
pub(super) fn run(command: &mut Command) -> Result<(), Box<dyn Error>> {
    eprintln!("Running {command:?}");
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{command:?} exited with {status}").into());
    }
    Ok(())
}

pub(super) fn output(command: &mut Command) -> Result<String, Box<dyn Error>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!("{command:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
