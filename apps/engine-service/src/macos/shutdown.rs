//! When the macOS service stops, and how it reports the stop to the controller. Graph and
//! recording retirement completes before independent Driver audio stops.
use crate::shutdown::collect;
use std::{future::Future, io, path::Path, time::Duration};

/// Resolves at an OS shutdown request or when the controller releases the engine,
/// whichever comes first.
pub(super) async fn first_stop(
    signal: impl Future<Output = io::Result<()>>,
    controller_released: impl Future<Output = ()>,
) -> io::Result<()> {
    tokio::select! {
        result = signal => result,
        () = controller_released => Ok(()),
    }
}

pub(super) async fn wait_for_controller(path: impl AsRef<Path>) {
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        interval.tick().await;
        match mixrelay_macos_transport::app_lifecycle::is_owned(&path) {
            Ok(true) => continue,
            Ok(false) => tracing::info!("controller released ownership; finalizing audio server"),
            Err(error) => {
                tracing::error!(%error, "controller lease probe failed; stopping audio server");
            }
        }
        return;
    }
}

pub(super) async fn finish(
    runtime: Result<(), String>,
    retired: Result<(), String>,
    stop_driver: impl Future<Output = Result<(), String>>,
) -> Result<(), String> {
    // Driver stop runs after retirement, including when earlier owners failed.
    collect([
        ("Engine", runtime),
        ("Native retirement", retired),
        ("Driver shutdown failed", stop_driver.await),
    ])
}

/// The engine lease's final record. The controller treats exactly `stopped` as a clean
/// exit and shows the errors after `failed` in its shutdown warning.
pub(super) fn exit_record(result: &Result<(), String>) -> String {
    result
        .as_ref()
        .map_or_else(|error| format!("failed\n{error}"), |()| "stopped".into())
}

#[cfg(test)]
mod tests;
