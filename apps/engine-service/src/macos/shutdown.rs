//! Complete graph/recording retirement before stopping independent Driver audio.
use std::{future::Future, path::Path, time::Duration};

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

pub(super) use crate::shutdown::collect;

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

#[cfg(test)]
mod tests;
