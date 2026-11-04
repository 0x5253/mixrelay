//! Tauri connection commands and native runtime adapters.
use super::{Connection, EXCHANGE_TIMEOUT, spawn_controller};
use mixrelay_desktop_core::runtime;
pub(crate) use runtime::{ConnectStage, RuntimeStatus};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
use tauri::Manager as _;
const START_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn compatibility() -> runtime::Compatibility<'static> {
    runtime::Compatibility {
        protocol: crate::control_channel::LINK_PROTOCOL,
        version: env!("CARGO_PKG_VERSION"),
    }
}

impl runtime::RuntimeEndpoint for Connection {
    async fn hello(&self) -> Option<Value> {
        self.invoke("hello", Value::Null).await.ok()
    }
    async fn engine_ready(&self) -> bool {
        self.invoke("control_get_status", Value::Null)
            .await
            .is_ok_and(|response| response["status"] == 200)
    }
}

async fn probe(connection: &Connection, engine_pending: impl FnOnce()) -> runtime::Probe {
    runtime::probe(
        connection,
        &compatibility(),
        &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
        EXCHANGE_TIMEOUT,
        engine_pending,
    )
    .await
}
async fn readiness(connection: &Connection) -> RuntimeStatus {
    probe(connection, || {}).await.status()
}

#[tauri::command]
pub(crate) async fn runtime_status(app: tauri::AppHandle) -> Result<RuntimeStatus, String> {
    let connection = app.state::<Connection>();
    let status = readiness(&connection).await;
    let status = if status == RuntimeStatus::Unavailable && !driver_executable().is_file() {
        RuntimeStatus::NotInstalled
    } else {
        status
    };
    publish_readiness(&app, status);
    Ok(status)
}

#[tauri::command]
pub(crate) async fn runtime_connect(
    app: tauri::AppHandle,
    on_stage: tauri::ipc::Channel<ConnectStage>,
) -> Result<RuntimeStatus, String> {
    let report = |stage: ConnectStage| {
        // Only a closed editor window drops the channel; the connection itself continues.
        if let Err(error) = on_stage.send(stage) {
            tracing::debug!(%error, ?stage, "could not report the connection stage");
        }
    };
    report(ConnectStage::Checking);
    let _connect = connection_gate(&app).lock_owned().await;
    tracing::info!("connecting to the Driver runtime");
    let connection = app.state::<Connection>();
    let status = probe(&connection, || report(ConnectStage::PreparingEngine))
        .await
        .status();
    tracing::info!(?status, "Driver runtime readiness before launch");
    if status != RuntimeStatus::Unavailable {
        tracing::info!(?status, "Driver runtime already answering");
        publish_readiness(&app, status);
        return Ok(status);
    }
    publish_readiness(&app, status);
    let executable = driver_executable();
    if !executable.is_file() {
        tracing::warn!(path = %executable.display(), "Driver runtime is not installed");
        return Ok(RuntimeStatus::NotInstalled);
    }
    #[cfg(target_os = "macos")]
    if !tauri::is_dev() {
        let version = installed_runtime_version().await;
        if version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
            tracing::warn!(
                expected = env!("CARGO_PKG_VERSION"),
                installed = version.as_deref().unwrap_or("unreadable"),
                "installed Driver runtime version is not supported"
            );
            return Ok(RuntimeStatus::Incompatible);
        }
    }
    let status = runtime::connect_until_ready(
        connection.inner(),
        &Launcher {
            executable: &executable,
            admission: controller_admission_available,
        },
        &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
        runtime::ConnectOptions {
            compatibility: compatibility(),
            timeout: START_TIMEOUT,
            exchange_timeout: EXCHANGE_TIMEOUT,
        },
        report,
    )
    .await
    .inspect_err(|error| tracing::error!(%error, "Driver runtime could not be started"))
    .map_err(|error| error.to_string())?;
    if status != RuntimeStatus::Ready {
        tracing::warn!(?status, "Driver runtime did not become ready");
    }
    publish_readiness(&app, status);
    Ok(status)
}

fn connection_gate(app: &tauri::AppHandle) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    std::sync::Arc::clone(&app.state::<Connection>().connect_gate)
}

impl runtime::LaunchObservation for super::ControllerLaunch {
    fn exited(&self) -> bool {
        self.exited()
    }
}
struct Launcher<'a, A> {
    executable: &'a std::path::Path,
    admission: A,
}
impl<A: Fn() -> std::io::Result<bool>> runtime::ControllerLauncher for Launcher<'_, A> {
    type Launch = super::ControllerLaunch;
    fn admission_available(&self) -> std::io::Result<bool> {
        (self.admission)()
    }
    fn launch(&self) -> std::io::Result<Self::Launch> {
        spawn_controller(self.executable)
    }
}

#[cfg_attr(
    not(target_os = "macos"),
    expect(
        clippy::unnecessary_wraps,
        reason = "the shared admission contract can fail while inspecting macOS leases"
    )
)]
fn controller_admission_available() -> std::io::Result<bool> {
    #[cfg(target_os = "macos")]
    if !tauri::is_dev() {
        use mixrelay_macos_transport::app_lifecycle::{CONTROLLER_LEASE, ENGINE_LEASE, is_owned};
        return Ok(!is_owned(CONTROLLER_LEASE)? && !is_owned(ENGINE_LEASE)?);
    }
    Ok(true)
}

fn publish_readiness(app: &tauri::AppHandle, status: RuntimeStatus) {
    if let Some(menu) = app.try_state::<crate::workspace_menu::WorkspaceMenu>()
        && let Err(error) = menu.set_ready(status == RuntimeStatus::Ready)
    {
        tracing::error!(%error, "could not update editor menu readiness");
    }
}

fn driver_executable() -> PathBuf {
    #[cfg(target_os = "macos")]
    return PathBuf::from(mixrelay_macos_transport::install_contract::APP_EXECUTABLE_PATH);
    #[cfg(target_os = "linux")]
    return PathBuf::from("/usr/bin/mixrelay-desktop");
}

pub(crate) fn editor_executable() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let path = PathBuf::from(mixrelay_macos_transport::install_contract::APP_EXECUTABLE_PATH);
    #[cfg(target_os = "linux")]
    let path = PathBuf::from("/usr/bin/mixrelay-desktop");
    if path.is_file() {
        Ok(path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "MixRelay is not installed",
        ))
    }
}

#[cfg(target_os = "macos")]
async fn installed_runtime_version() -> Option<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let output = std::process::Command::new("/usr/bin/plutil")
            .args([
                "-extract",
                "CFBundleShortVersionString",
                "raw",
                "-o",
                "-",
                mixrelay_macos_transport::install_contract::APP_INFO_PATH,
            ])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests;
