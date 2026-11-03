use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::native_text::{
    Locale, current_locale,
    dialogs::{Text, detail, text},
};
use mixrelay_macos_transport::app_lifecycle::{
    CONTROLLER_LEASE, ENGINE_LEASE, EngineWake, ProcessLease, engine_exit_record, is_owned,
};
use tauri::Manager;

/// Background controller ownership, independent of window visibility and graph power state.
pub(crate) struct EngineLifecycle {
    owner: Mutex<Option<ControllerOwner>>,
    quitting: AtomicBool,
    finished: AtomicBool,
    driver_transition: tokio::sync::Mutex<()>,
    driver_start_cancelled: AtomicBool,
    watching: AtomicBool,
}

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
        Ok(Self {
            owner: Mutex::new(Some(ControllerOwner {
                wake: Some(wake),
                lease: Some(lease),
            })),
            quitting: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            driver_transition: tokio::sync::Mutex::const_new(()),
            driver_start_cancelled: AtomicBool::new(false),
            watching: AtomicBool::new(false),
        })
    }

    pub(crate) async fn restore_driver(
        &self,
        prepare: impl FnOnce() -> Result<
            mixrelay_driver_adapters::DriverClient,
            mixrelay_driver_core::DriverError,
        > + Send,
    ) -> Result<Option<mixrelay_driver_core::DriverSnapshot>, String> {
        let _transition = self.driver_transition.lock().await;
        if self.driver_start_cancelled.load(Ordering::Acquire) {
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
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        api.prevent_exit();
        if self.quitting.swap(true, Ordering::AcqRel) {
            return;
        }
        self.driver_start_cancelled.store(true, Ordering::Release);
        app.state::<crate::status_menu::StatusMenu>()
            .set_runtime_stopping(true);
        app.state::<crate::control_proxy::ControlProxyClient>()
            .close_admission();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            match finish_local_controls(&app).await {
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
                    tracing::error!(%error, "audio server exit is not confirmed");
                    crate::status_menu::shutdown_failed(&app).await;
                }
            }
        });
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
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut absence = Absence::default();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let lifecycle = app.state::<EngineLifecycle>();
            if lifecycle.finished.load(Ordering::Acquire) {
                return;
            }
            if lifecycle.stopping() {
                continue;
            }
            let owned = match is_owned(ENGINE_LEASE) {
                Ok(owned) => owned,
                Err(error) => {
                    tracing::warn!(%error, "could not observe the audio engine");
                    continue;
                }
            };
            match absence.observe(owned) {
                Watch::Running | Watch::Waiting => {}
                Watch::Lost { unexpected } => {
                    if unexpected {
                        tracing::warn!("audio engine stopped unexpectedly; starting it again");
                    }
                    if let Err(error) =
                        crate::control_proxy::release_lost_engine_captures(&app).await
                    {
                        tracing::error!(%error, "could not release captures of the lost engine");
                    }
                }
                Watch::Wake => lifecycle.wake_engine(),
                Watch::Recovered => {
                    tracing::info!("audio engine is running; restoring app captures");
                    crate::restore_captures(&app);
                }
            }
        }
    });
}

impl EngineLifecycle {
    /// A stop request, even one whose shutdown failed and may be retried, means the
    /// engine is no longer wanted.
    fn stopping(&self) -> bool {
        self.quitting.load(Ordering::Acquire) || self.driver_start_cancelled.load(Ordering::Acquire)
    }

    fn wake_engine(&self) {
        let Ok(mut owner) = self.owner.lock() else {
            return;
        };
        // Rechecked under the owner lock; a completed quit has already taken the owner.
        let Some(owner) = owner.as_mut().filter(|_| !self.stopping()) else {
            return;
        };
        match EngineWake::start() {
            // The previous connection is cancelled only after the new request is queued.
            Ok(wake) => owner.wake = Some(wake),
            Err(error) => tracing::error!(%error, "could not start the audio engine again"),
        }
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
    app.state::<EngineLifecycle>()
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

use mixrelay_desktop_core::lifecycle::{
    Absence, ExitObservation, Watch, is_app_owned_configuration,
};

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
    await_engine_exit(observe_engine_exit, || {
        tracing::warn!("waiting for native audio server teardown; exit remains pending");
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_title(text(current_locale(), Text::EngineStoppingTitle));
        }
    })
    .await
}

async fn await_engine_exit(
    observe: impl AsyncFnMut() -> Result<ExitObservation<String>, String>,
    waiting: impl FnMut(),
) -> Result<Option<String>, String> {
    let record = mixrelay_desktop_core::lifecycle::await_engine_exit(
        &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
        observe,
        waiting,
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
        let lifecycle = EngineLifecycle {
            owner: Mutex::new(None),
            quitting: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            driver_transition: tokio::sync::Mutex::const_new(()),
            driver_start_cancelled: AtomicBool::new(false),
            watching: AtomicBool::new(false),
        };
        let in_flight = lifecycle.driver_transition.lock().await;
        let queued =
            lifecycle.restore_driver(|| panic!("cancelled start must not create a client"));
        tokio::pin!(queued);
        assert!(
            tokio::time::timeout(Duration::from_millis(1), &mut queued)
                .await
                .is_err()
        );
        lifecycle
            .driver_start_cancelled
            .store(true, Ordering::Release);
        // The quit-in-progress flag resets on a failed stop, allowing retry.
        // That must not restore permission for the old queued startup.
        lifecycle.quitting.store(false, Ordering::Release);
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
