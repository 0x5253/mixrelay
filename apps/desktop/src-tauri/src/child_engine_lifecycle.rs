use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::native_text::{
    current_locale,
    dialogs::{Text, detail, text},
};
use mixrelay_desktop_application::timing::AsyncDelay as _;
use mixrelay_engine_process::ControllerEngine;
use mixrelay_time::MonotonicClock as _;
use tauri::Manager;

/// Owns only this controller's bundled engine, not an existing localhost listener.
pub(crate) struct EngineLifecycle {
    engine: Mutex<ControllerEngine>,
    quitting: AtomicBool,
    finished: AtomicBool,
}

impl EngineLifecycle {
    pub(crate) fn start() -> Result<Self, String> {
        tauri::async_runtime::block_on(crate::linux_service::start_driver())
            .map_err(|error| error.to_string())?;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let path = bundled_engine_path(&executable)?;
        let engine = ControllerEngine::start(&path, &mixrelay_time::StdClock::new())
            .map_err(|error| detail(current_locale(), Text::BundledEngineStartFailed, error))?;
        Ok(Self {
            engine: Mutex::new(engine),
            quitting: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        })
    }

    pub(crate) fn exit_requested(&self, app: &tauri::AppHandle, api: &tauri::ExitRequestApi) {
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        api.prevent_exit();
        if self.quitting.swap(true, Ordering::AcqRel) {
            return;
        }
        app.state::<crate::control_proxy::ControlProxyClient>()
            .close_admission();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match finish_engine(&app).await {
                Ok(warning) => {
                    if let Some(warning) = warning {
                        rfd::AsyncMessageDialog::new()
                            .set_title("MixRelay")
                            .set_description(&warning)
                            .set_level(rfd::MessageLevel::Error)
                            .set_buttons(rfd::MessageButtons::OkCustom(
                                text(current_locale(), Text::Ok).into(),
                            ))
                            .show()
                            .await;
                    }
                    app.state::<Self>().finished.store(true, Ordering::Release);
                    app.exit(0);
                }
                Err(error) => {
                    app.state::<Self>().quitting.store(false, Ordering::Release);
                    tracing::error!(%error, "owned audio engine shutdown failed");
                    crate::status_menu::shutdown_failed(&app).await;
                }
            }
        });
    }
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
            || {
                lifecycle.finished.load(Ordering::Acquire)
                    || lifecycle.quitting.load(Ordering::Acquire)
            },
            || {
                lifecycle
                    .engine
                    .lock()
                    .map_err(|_| text(current_locale(), Text::EngineOwnershipLockFailed).to_owned())
                    .and_then(|mut engine| {
                        engine
                            .try_wait()
                            .map(|status| status.is_some())
                            .map_err(|error| error.to_string())
                    })
            },
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

async fn restart_owned_engine(app: &tauri::AppHandle) -> Result<bool, String> {
    let lifecycle = app.state::<EngineLifecycle>();
    let client = app.state::<crate::control_proxy::ControlProxyClient>();
    client.drain().await?;
    if lifecycle.quitting.load(Ordering::Acquire) {
        return Ok(false);
    }
    {
        let mut engine = lifecycle
            .engine
            .lock()
            .map_err(|_| text(current_locale(), Text::EngineOwnershipLockFailed).to_owned())?;
        if engine
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(text(current_locale(), Text::PreviousEngineRunning).to_owned());
        }
    }
    crate::linux_service::start_driver()
        .await
        .map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let path = bundled_engine_path(&executable)?;
    let replacement = tauri::async_runtime::spawn_blocking(move || {
        ControllerEngine::start(&path, &mixrelay_time::StdClock::new())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    {
        let mut engine = lifecycle
            .engine
            .lock()
            .map_err(|_| text(current_locale(), Text::EngineOwnershipLockFailed).to_owned())?;
        if lifecycle.quitting.load(Ordering::Acquire) {
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
    {
        let lifecycle = app.state::<EngineLifecycle>();
        lifecycle
            .engine
            .lock()
            .map_err(|_| text(current_locale(), Text::EngineOwnershipLockFailed))?
            .request_shutdown();
    }
    let timer = crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new());
    let mut notice = mixrelay_desktop_application::lifecycle::ExitNotice::new(&timer);
    loop {
        let status = app
            .state::<EngineLifecycle>()
            .engine
            .lock()
            .map_err(|_| text(current_locale(), Text::EngineOwnershipLockFailed))?
            .try_wait()
            .map_err(|error| error.to_string())?;
        if let Some(status) = status {
            return Ok((!status.success())
                .then(|| detail(current_locale(), Text::EngineFinalizationFailed, status)));
        }
        if notice.due(&timer) {
            tracing::warn!("waiting for owned audio engine finalization");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(text(current_locale(), Text::EngineStoppingTitle));
            }
        }
        timer
            .wait_until(mixrelay_time::Deadline::after(
                timer.now(),
                Duration::from_millis(100),
            ))
            .await;
    }
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
