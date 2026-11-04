//! The macOS controller owns audio while it runs: launchd starts the engine on demand,
//! and this controller wakes it again if it dies, until Quit confirms teardown.
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::native_text::{
    Locale, current_locale,
    dialogs::{Text, detail, text},
};
use mixrelay_desktop_core::lifecycle::{
    ExitObservation, OnDemandEngine, is_app_owned_configuration, shutdown::ShutdownGate,
};
use mixrelay_macos_transport::app_lifecycle::{
    CONTROLLER_LEASE, ENGINE_LEASE, EngineWake, ProcessLease, engine_exit_record, is_owned,
};
use tauri::Manager;

/// Background controller ownership, independent of window visibility and graph power state.
pub(crate) struct EngineLifecycle {
    owner: Mutex<Option<ControllerOwner>>,
    shutdown: Arc<ShutdownGate>,
    driver_transition: tokio::sync::Mutex<()>,
    watching: AtomicBool,
}

/// Dropping the owner cancels the engine wake request, then releases the controller lease.
struct ControllerOwner {
    wake: Option<EngineWake>,
    lease: Option<ProcessLease>,
}

impl Drop for ControllerOwner {
    fn drop(&mut self) {
        self.wake.take();
        self.lease.take();
    }
}

impl EngineLifecycle {
    pub(crate) fn start() -> Result<Self, String> {
        validate_installation()?;
        let lease = ProcessLease::acquire(CONTROLLER_LEASE)
            .map_err(|error| detail(current_locale(), Text::DesktopOwnershipFailed, error))?;
        if is_owned(ENGINE_LEASE).map_err(|error| error.to_string())? {
            return Err(text(current_locale(), Text::AudioServerShuttingDownRetry).into());
        }
        let wake = EngineWake::start().map_err(|error| error.to_string())?;
        Ok(Self::owning(Some(ControllerOwner {
            wake: Some(wake),
            lease: Some(lease),
        })))
    }

    fn owning(owner: Option<ControllerOwner>) -> Self {
        Self {
            owner: Mutex::new(owner),
            shutdown: Arc::default(),
            driver_transition: tokio::sync::Mutex::const_new(()),
            watching: AtomicBool::new(false),
        }
    }

    /// Activates the Driver unless a stop was requested before this start got its turn.
    pub(crate) async fn restore_driver(
        &self,
        prepare: impl FnOnce() -> Result<
            mixrelay_driver_adapters::DriverClient,
            mixrelay_driver_core::DriverError,
        > + Send,
    ) -> Result<Option<mixrelay_driver_core::DriverSnapshot>, String> {
        let _transition = self.driver_transition.lock().await;
        if self.shutdown.requested() {
            return Ok(None);
        }
        prepare()
            .map_err(|error| error.to_string())?
            .set_active(true)
            .await
            .map(Some)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn exit_requested(&self, app: &tauri::AppHandle, api: &tauri::ExitRequestApi) {
        crate::engine_exit::on_exit_requested(
            app,
            api,
            &self.shutdown,
            || {
                app.state::<crate::status_menu::StatusMenu>()
                    .set_runtime_stopping(true);
                app.state::<crate::control_proxy::ControlProxyClient>()
                    .close_admission();
                let app = app.clone();
                async move { finish_local_controls(&app).await }
            },
            "audio server exit is not confirmed",
        );
    }

    fn wake_engine(&self) {
        let Ok(mut owner) = self.owner.lock() else {
            return;
        };
        // Rechecked under the owner lock; a completed quit has already taken the owner.
        let Some(owner) = owner.as_mut().filter(|_| !self.shutdown.requested()) else {
            return;
        };
        match EngineWake::start() {
            // The previous connection is cancelled only after the new request is queued.
            Ok(wake) => owner.wake = Some(wake),
            Err(error) => tracing::error!(%error, "could not start the audio engine again"),
        }
    }
}

/// launchd starts the engine on demand but never restarts it (`KeepAlive` is false, so
/// a stopped app cannot leave it running). While this controller owns audio, an engine
/// that dies is woken again; it restores its saved graph and pause on start, and app
/// captures bound to the lost engine are rebuilt for its successor.
pub(crate) fn monitor(app: &tauri::AppHandle) {
    if app
        .state::<EngineLifecycle>()
        .watching
        .swap(true, Ordering::AcqRel)
    {
        return;
    }
    let engine = WatchedEngine { app: app.clone() };
    tauri::async_runtime::spawn(async move {
        mixrelay_desktop_core::lifecycle::watch_on_demand_engine(
            &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
            &engine,
        )
        .await;
    });
}

struct WatchedEngine {
    app: tauri::AppHandle,
}

impl OnDemandEngine for WatchedEngine {
    type Error = std::io::Error;

    fn shutdown_finished(&self) -> bool {
        self.app.state::<EngineLifecycle>().shutdown.finished()
    }

    fn stop_requested(&self) -> bool {
        self.app.state::<EngineLifecycle>().shutdown.requested()
    }

    fn is_running(&self) -> Result<bool, Self::Error> {
        is_owned(ENGINE_LEASE)
    }

    fn observation_failed(&self, error: Self::Error) {
        tracing::warn!(%error, "could not observe the audio engine");
    }

    async fn lost(&self, unexpected: bool) {
        if unexpected {
            tracing::warn!("audio engine stopped unexpectedly; starting it again");
        }
        if let Err(error) = crate::control_proxy::release_lost_engine_captures(&self.app).await {
            tracing::error!(%error, "could not release captures of the lost engine");
        }
    }

    fn wake(&self) {
        self.app.state::<EngineLifecycle>().wake_engine();
    }

    fn recovered(&self) {
        tracing::info!("audio engine is running; restoring app captures");
        crate::background::restore_captures(&self.app);
    }
}

async fn finish_local_controls(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    tracing::info!(phase = "local_socket", "draining runtime controls for Quit");
    app.state::<crate::midi::Midi>().shutdown().await?;
    app.state::<crate::control_proxy::local::socket::LocalSocket>()
        .shutdown()
        .await?;
    tracing::info!(
        phase = "engine_requests",
        "draining runtime controls for Quit"
    );
    app.state::<crate::control_proxy::ControlProxyClient>()
        .drain()
        .await?;
    // Native AppKit Quit and a system background-stop request enter here too,
    // without going through the status menu's stop_all command. Stop and save
    // the HAL inventory before releasing the controller; otherwise its audio return path
    // outlives both the controller and the graph engine.
    let lifecycle = app.state::<EngineLifecycle>();
    let _transition = lifecycle.driver_transition.lock().await;
    tracing::info!(phase = "driver", "stopping audio for Quit");
    crate::driver_control::stop_driver().await?;
    lifecycle
        .owner
        .lock()
        .map_err(|_| text(current_locale(), Text::DesktopLifecycleLockFailed).to_owned())?
        .take();
    tracing::info!(phase = "engine_exit", "waiting for audio teardown for Quit");
    wait_for_engine_exit(app).await
}

fn validate_installation() -> Result<(), String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args([
            "-convert",
            "json",
            "-o",
            "-",
            "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist",
        ])
        .output()
        .map_err(|error| error.to_string())?;
    let config: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| text(current_locale(), Text::AudioServiceInvalid).to_owned())?;
    if !is_app_owned_configuration(&config) {
        return Err(text(current_locale(), Text::AudioServiceIncompatible).into());
    }
    Ok(())
}

async fn observe_engine_exit() -> Result<ExitObservation<String>, String> {
    let owned = is_owned(ENGINE_LEASE).map_err(|error| error.to_string())?;
    // A connectable engine socket means the service is still serving its teardown.
    let socket = mixrelay_local_socket::session_socket_directory()
        .map(|directory| directory.join(mixrelay_local_socket::ENGINE_SOCKET))
        .map_err(|error| error.to_string())?;
    let connected = tokio::time::timeout(
        Duration::from_millis(250),
        tokio::net::UnixStream::connect(socket),
    )
    .await
    .is_ok_and(|result| result.is_ok());
    if owned || connected {
        return Ok(ExitObservation::Pending);
    }
    engine_exit_record()
        .map(ExitObservation::Stopped)
        .map_err(|error| error.to_string())
}

async fn wait_for_engine_exit(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let record = mixrelay_desktop_core::lifecycle::await_engine_exit(
        &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
        observe_engine_exit,
        || {
            tracing::warn!("waiting for native audio server teardown; exit remains pending");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(text(current_locale(), Text::EngineStoppingTitle));
            }
        },
    )
    .await?;
    Ok(exit_warning(&record))
}

fn exit_warning(record: &str) -> Option<String> {
    exit_warning_for(current_locale(), record)
}

fn exit_warning_for(locale: Locale, record: &str) -> Option<String> {
    if record == "stopped" {
        return None;
    }
    Some(format!(
        "{}\n{}",
        text(locale, Text::AudioServerShutdownWarning),
        record.strip_prefix("failed\n").unwrap_or(record)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn queued_start_cannot_revive_driver_after_a_stop_even_if_stop_needs_retry() {
        let lifecycle = EngineLifecycle::owning(None);
        let in_flight = lifecycle.driver_transition.lock().await;
        let queued =
            lifecycle.restore_driver(|| panic!("cancelled start must not create a client"));
        tokio::pin!(queued);
        assert!(
            tokio::time::timeout(Duration::from_millis(1), &mut queued)
                .await
                .is_err()
        );
        lifecycle.shutdown.request_exit();
        // A failed stop reopens Quit for a retry. That must not restore permission for
        // the old queued startup.
        lifecycle.shutdown.attempt_failed();
        drop(in_flight);
        assert!(queued.await.unwrap().is_none());
    }

    #[test]
    fn unfinished_or_failed_record_is_not_reported_as_clean_shutdown() {
        assert!(exit_warning("stopped").is_none());
        assert!(exit_warning("running").is_some());
        assert!(exit_warning("").is_some());
        assert!(
            exit_warning("failed\ndisk full")
                .expect("warning")
                .contains("disk full")
        );
    }

    #[test]
    fn failed_shutdown_warning_reports_the_record_without_claiming_data_loss() {
        assert_eq!(
            exit_warning_for(Locale::Ko, "failed\nCoreAudio worker panicked during retirement"),
            Some(
                "오디오 서버 종료 중 오류가 발생했습니다.\nCoreAudio worker panicked during retirement"
                    .into()
            )
        );
    }

    #[test]
    fn localized_shutdown_warning_preserves_the_diagnostic_record() {
        for locale in [
            Locale::En,
            Locale::Ko,
            Locale::Ja,
            Locale::ZhHans,
            Locale::ZhHant,
            Locale::Fr,
            Locale::De,
        ] {
            assert!(exit_warning_for(locale, "stopped").is_none());
            assert_eq!(
                exit_warning_for(locale, "failed\nCoreAudio worker stopped"),
                Some(format!(
                    "{}\nCoreAudio worker stopped",
                    text(locale, Text::AudioServerShutdownWarning)
                ))
            );
        }
    }
}
