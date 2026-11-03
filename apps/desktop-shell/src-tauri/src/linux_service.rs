//! Start the packaged device service in the same normal-user session before the engine.
use std::{future::Future, process::ExitStatus, time::Duration};

// Starting the existing unit is idempotent and must fit within explicit Connect's 30s budget.
const START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub(crate) enum StartError {
    #[error("could not start the MixRelay device service: {0}")]
    Io(#[from] std::io::Error),
    #[error("starting the MixRelay device service timed out")]
    Timeout,
    #[error("the user service manager refused to start MixRelay: {0}")]
    Refused(ExitStatus),
}

/// Starts only the installed user's unit; never enables it or changes saved audio state.
#[cfg(target_os = "linux")]
pub(crate) async fn start_driver() -> Result<(), StartError> {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new("systemctl")
        .args(["--user", "start", "mixrelay-driver.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let result = wait_for_start(child.wait()).await;
    if matches!(result, Err(StartError::Timeout)) {
        child.kill().await?;
    }
    result
}

async fn wait_for_start(
    completion: impl Future<Output = std::io::Result<ExitStatus>>,
) -> Result<(), StartError> {
    let status = tokio::time::timeout(START_TIMEOUT, completion)
        .await
        .map_err(|_| StartError::Timeout)??;
    if !status.success() {
        return Err(StartError::Refused(status));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
