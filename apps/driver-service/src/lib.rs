//! Installed Driver control owner. Explicit manager quit stops devices through its API;
//! the service remains reachable independently of manager and graph process lifetimes.

pub mod logging;
mod manager;
mod settings;

use std::ffi::OsString;

use anyhow::{Context, Result};
use mixrelay_local_socket::{
    AuthorizedListener, DRIVER_SOCKET, bind, current_uid, socket_directory,
};

/// Serves the Driver control API until an interrupt or termination request.
///
/// # Errors
/// Returns invalid arguments, socket admission, native Driver startup or serving failure.
pub async fn run(arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let path = settings::settings_path(arguments)?;
    // Reserve admission before any native inventory can become visible. A duplicate process
    // must fail here rather than publish another owner of the same device identities.
    let uid = current_uid();
    let (listener, socket) = bind(&socket_directory()?, DRIVER_SOCKET, uid)
        .await
        .context("reserve Driver control socket")?;
    // Root is admitted for the macOS installer's readiness probe of the installed user's service.
    let listener = AuthorizedListener::new(listener, socket, vec![uid, 0]);
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("observe Driver service termination")?;
    let manager = tokio::task::spawn_blocking(move || manager::open_manager(path))
        .await
        .context("initialize native Driver owner")??;
    tracing::info!("Driver control API starting; native readiness is reported separately");
    axum::serve(
        listener,
        mixrelay_driver_adapters::router(
            manager,
            mixrelay_driver_adapters::InventoryIntervals::default(),
        )
        .into_make_service_with_connect_info::<mixrelay_local_socket::Peer>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::error!(%error, "Driver interrupt observer failed");
                }
            }
            _ = terminate.recv() => {}
        }
    })
    .await
    .context("serve Driver control requests")?;
    // Graceful shutdown closes request admission and awaits native control transactions.
    // Dropping its final backend drains PipeWire callbacks. The HAL independently owns its
    // PCM mappings; macOS control-only descriptors close with this service process.
    Ok(())
}
