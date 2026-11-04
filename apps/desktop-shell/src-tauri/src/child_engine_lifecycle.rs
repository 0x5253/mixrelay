//! The Linux controller owns its bundled engine as a child process and replaces it when it
//! exits, until Quit confirms teardown.
use std::{
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::native_text::{
    current_locale,
    dialogs::{Text, detail, text},
};
use mixrelay_desktop_core::lifecycle::{ExitObservation, shutdown::ShutdownGate};
use mixrelay_engine_supervisor::ControllerEngine;
use tauri::Manager;

/// Owns only this controller's bundled engine, not an existing localhost listener.
pub(crate) struct EngineLifecycle {
    engine: Mutex<ControllerEngine>,
    shutdown: Arc<ShutdownGate>,
}

impl EngineLifecycle {
    pub(crate) fn start() -> Result<Self, String> {
        tauri::async_runtime::block_on(crate::linux_service::start_driver())
            .map_err(|error| error.to_string())?;
        let engine =
            ControllerEngine::start(&bundled_engine()?, &mixrelay_time::StdClock::new())
                .map_err(|error| detail(current_locale(), Text::BundledEngineStartFailed, error))?;
        Ok(Self {
            engine: Mutex::new(engine),
            shutdown: Arc::default(),
        })
    }

    pub(crate) fn exit_requested(&self, app: &tauri::AppHandle, api: &tauri::ExitRequestApi) {
        crate::engine_exit::on_exit_requested(
            app,
            api,
            &self.shutdown,
            || {
                app.state::<crate::control_proxy::ControlProxyClient>()
                    .close_admission();
                let app = app.clone();
                async move { finish_engine(&app).await }
            },
            "owned audio engine shutdown failed",
        );
    }

    fn engine(&self) -> Result<MutexGuard<'_, ControllerEngine>, String> {
        self.engine
            .lock()
            .map_err(|_| text(current_locale(), Text::EngineOwnershipLockFailed).to_owned())
    }

    /// The owned engine's exit status, or `None` while it runs.
    fn engine_exit(&self) -> Result<Option<ExitStatus>, String> {
        self.engine()?.try_wait().map_err(|error| error.to_string())
    }
}

fn bundled_engine() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    bundled_engine_path(&executable)
}

fn bundled_engine_path(desktop: &Path) -> Result<PathBuf, String> {
    let directory = desktop
        .parent()
        .ok_or_else(|| text(current_locale(), Text::DesktopExecutableParentMissing))?;
    let engine = directory.join("mixrelay-service");
    if !engine.is_file() {
        return Err(detail(
            current_locale(),
            Text::BundledEngineMissing,
            engine.display(),
        ));
    }
    Ok(engine)
}

/// Readiness is a point-in-time acknowledgement, not a promise of future life.
/// Suspend commands while replacing an exited child with a new owned engine.
/// The successor restores its durable graph and pause before acknowledging readiness.
pub(crate) fn monitor(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let lifecycle = app.state::<EngineLifecycle>();
        crate::engine_supervision::supervise(
            || lifecycle.shutdown.stopping(),
            || lifecycle.engine_exit().map(|status| status.is_some()),
            || {
                let app = app.clone();
                async move {
                    app.state::<crate::control_proxy::ControlProxyClient>()
                        .suspend_admission();
                    restart_owned_engine(&app).await.map(|_| ())
                }
            },
        )
        .await;
    });
}

/// Replaces the exited engine unless Quit began meanwhile; returns whether commands resumed.
async fn restart_owned_engine(app: &tauri::AppHandle) -> Result<bool, String> {
    let lifecycle = app.state::<EngineLifecycle>();
    let client = app.state::<crate::control_proxy::ControlProxyClient>();
    client.drain().await?;
    if lifecycle.shutdown.stopping() {
        return Ok(false);
    }
    if lifecycle.engine_exit()?.is_none() {
        return Err(text(current_locale(), Text::PreviousEngineRunning).to_owned());
    }
    crate::linux_service::start_driver()
        .await
        .map_err(|error| error.to_string())?;
    let path = bundled_engine()?;
    let replacement = tauri::async_runtime::spawn_blocking(move || {
        ControllerEngine::start(&path, &mixrelay_time::StdClock::new())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    {
        let mut engine = lifecycle.engine()?;
        if lifecycle.shutdown.stopping() {
            // Dropping the uninstalled child closes only its private shutdown lease.
            return Ok(false);
        }
        *engine = replacement;
    }
    Ok(client.resume_admission().await)
}

async fn finish_engine(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    app.state::<crate::midi::Midi>().shutdown().await?;
    app.state::<crate::control_proxy::local::socket::LocalSocket>()
        .shutdown()
        .await?;
    app.state::<crate::control_proxy::ControlProxyClient>()
        .drain()
        .await?;
    let lifecycle = app.state::<EngineLifecycle>();
    lifecycle.engine()?.request_shutdown();
    let status =
        mixrelay_desktop_core::lifecycle::await_engine_exit(
            &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
            || {
                std::future::ready(lifecycle.engine_exit().map(|status| {
                    status.map_or(ExitObservation::Pending, ExitObservation::Stopped)
                }))
            },
            || {
                tracing::warn!("waiting for owned audio engine finalization");
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_title(text(current_locale(), Text::EngineStoppingTitle));
                }
            },
        )
        .await?;
    Ok((!status.success())
        .then(|| detail(current_locale(), Text::EngineFinalizationFailed, status)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_only_the_adjacent_bundled_engine() {
        let directory = tempfile::tempdir().unwrap();
        let desktop = directory.path().join("MixRelay");
        assert!(bundled_engine_path(&desktop).is_err());
        let expected = directory.path().join("mixrelay-service");
        std::fs::write(&expected, b"fixture").unwrap();
        assert_eq!(bundled_engine_path(&desktop).unwrap(), expected);
    }
}
