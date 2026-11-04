//! Tauri connection commands and native runtime adapters.
use super::{Connection, EXCHANGE_TIMEOUT, spawn_controller};
use mixrelay_desktop_core::runtime;
pub(crate) use runtime::{ConnectStage, RuntimeStatus};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
use tauri::Manager as _;
const START_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
pub(super) fn compatible(hello: &Value) -> bool {
    compatibility().accepts(hello)
}
fn compatibility() -> runtime::Compatibility<'static> {
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
    let status = connect_until_ready(
        &connection,
        &executable,
        START_TIMEOUT,
        controller_admission_available,
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

async fn connect_until_ready(
    connection: &Connection,
    executable: &std::path::Path,
    timeout: Duration,
    admission: impl Fn() -> std::io::Result<bool>,
    report: impl Fn(ConnectStage),
) -> std::io::Result<RuntimeStatus> {
    runtime::connect_until_ready(
        connection,
        &Launcher {
            executable,
            admission,
        },
        &crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new()),
        runtime::ConnectOptions {
            compatibility: compatibility(),
            timeout,
            exchange_timeout: EXCHANGE_TIMEOUT,
        },
        report,
    )
    .await
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
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::MetadataExt as _;
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
        net::UnixListener,
    };

    async fn serve_connection(listener: UnixListener, status: u16) {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut frame = String::new();
            reader.read_line(&mut frame).await.unwrap();
            let request: Value = serde_json::from_str(&frame).unwrap();
            let body = if request["command"]["name"] == "hello" {
                json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")})
            } else {
                json!({"status":status})
            };
            reader
                .get_mut()
                .write_all(
                    format!(
                        "{}\n",
                        json!({"id":request["id"],"status":200,"body":{"ok":body}})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    }

    fn fixture(directory: &std::path::Path, script: &str) -> (Connection, PathBuf) {
        let executable = directory.join("controller.sh");
        mixrelay_test_support::write_executable(
            &executable,
            &format!("#!/bin/sh\ncd \"$(dirname \"$0\")\"\n{script}\n"),
        )
        .unwrap();
        (
            Connection::for_socket_fixture(
                directory.join("control.sock"),
                std::fs::metadata(directory).unwrap().uid(),
            ),
            executable,
        )
    }

    #[tokio::test]
    async fn exited_first_launch_is_retried_during_the_same_explicit_connection() {
        let directory = tempfile::tempdir().unwrap();
        let (connection, executable) = fixture(
            directory.path(),
            "printf attempt >> attempts\nif test ! -f first; then touch first; exit 2; fi\ntouch ready",
        );
        let path = directory.path().join("control.sock");
        let ready = directory.path().join("ready");
        let server = tokio::spawn(async move {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            serve_connection(UnixListener::bind(path).unwrap(), 200).await;
        });
        let stages = std::sync::Mutex::new(Vec::new());
        assert_eq!(
            connect_until_ready(
                &connection,
                &executable,
                Duration::from_secs(4),
                || Ok(true),
                |stage| stages.lock().unwrap().push(stage)
            )
            .await
            .unwrap(),
            RuntimeStatus::Ready
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("attempts")).unwrap(),
            "attemptattempt"
        );
        // A retried launch is still one Starting step, then its answering runtime starts the engine.
        assert_eq!(
            *stages.lock().unwrap(),
            [ConnectStage::Starting, ConnectStage::PreparingEngine]
        );
        server.abort();
    }

    #[tokio::test]
    async fn busy_owner_and_a_living_failed_engine_are_never_duplicated() {
        let directory = tempfile::tempdir().unwrap();
        let (connection, executable) = fixture(directory.path(), "touch launched");
        assert_eq!(
            connect_until_ready(
                &connection,
                &executable,
                Duration::from_millis(100),
                || Ok(false),
                |_| {}
            )
            .await
            .unwrap(),
            RuntimeStatus::Unavailable
        );
        assert!(!directory.path().join("launched").exists());
        let server = tokio::spawn(serve_connection(
            UnixListener::bind(directory.path().join("control.sock")).unwrap(),
            503,
        ));
        let stages = std::sync::Mutex::new(Vec::new());
        assert_eq!(
            connect_until_ready(
                &connection,
                &executable,
                Duration::from_millis(100),
                || Ok(true),
                |stage| stages.lock().unwrap().push(stage)
            )
            .await
            .unwrap(),
            RuntimeStatus::Unavailable
        );
        assert!(!directory.path().join("launched").exists());
        // The answering runtime owns its engine start; it is reported once, never relaunched.
        assert_eq!(*stages.lock().unwrap(), [ConnectStage::PreparingEngine]);
        server.abort();
    }

    #[tokio::test]
    async fn a_slow_but_healthy_engine_is_reported_as_being_prepared() {
        let directory = tempfile::tempdir().unwrap();
        let (connection, executable) = fixture(directory.path(), "touch launched");
        let listener = UnixListener::bind(directory.path().join("control.sock")).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            for (result, delay) in [
                (
                    json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")}),
                    0,
                ),
                (json!({"status":200}), 300),
            ] {
                let mut frame = String::new();
                reader.read_line(&mut frame).await.unwrap();
                let request: Value = serde_json::from_str(&frame).unwrap();
                tokio::time::sleep(Duration::from_millis(delay)).await;
                reader
                    .get_mut()
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"id":request["id"],"status":200,"body":{"ok":result}})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let stages = std::sync::Mutex::new(Vec::new());
        assert_eq!(
            connect_until_ready(
                &connection,
                &executable,
                Duration::from_secs(4),
                || Ok(true),
                |stage| stages.lock().unwrap().push(stage)
            )
            .await
            .unwrap(),
            RuntimeStatus::Ready
        );
        assert_eq!(*stages.lock().unwrap(), [ConnectStage::PreparingEngine]);
        assert!(!directory.path().join("launched").exists());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_broken_executable_has_a_fixed_retry_limit() {
        let directory = tempfile::tempdir().unwrap();
        let (connection, executable) =
            fixture(directory.path(), "printf attempt >> attempts\nexit 2");
        assert_eq!(
            connect_until_ready(
                &connection,
                &executable,
                Duration::from_secs(5),
                || Ok(true),
                |_| {}
            )
            .await
            .unwrap(),
            RuntimeStatus::Unavailable
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("attempts")).unwrap(),
            "attemptattemptattempt"
        );
    }

    #[test]
    fn old_app_controller_and_future_protocol_are_not_driver_connections() {
        for hello in [
            json!({"protocolVersion":1,"version":"0.1.2"}),
            json!({"protocolVersion":2,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")}),
            json!({"protocolVersion":2,"product":"mixrelay-app","version":"0.1.2"}),
            json!({"protocolVersion":4,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")}),
            json!({"protocolVersion":2,"product":"mixrelay-driver","version":"0.1.1"}),
            json!({"protocolVersion":2,"product":"mixrelay-driver","version":"0.1.2"}),
            json!({"protocolVersion":2,"product":"mixrelay-driver","version":"0.2.0"}),
        ] {
            assert!(!compatible(&hello));
        }
        assert!(compatible(
            &json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")})
        ));
    }

    #[tokio::test]
    async fn controller_with_unavailable_engine_never_opens_editor() {
        for code in [200, 503] {
            let directory = tempfile::tempdir().unwrap();
            let connection = Connection::for_socket_fixture(
                directory.path().join("control.sock"),
                std::fs::metadata(directory.path()).unwrap().uid(),
            );
            let listener = UnixListener::bind(directory.path().join("control.sock")).unwrap();
            let server = tokio::spawn(async move {
                // The controller keeps a connection open for the next request.
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                for result in [
                    json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")}),
                    json!({"status":code}),
                ] {
                    let mut frame = String::new();
                    reader.read_line(&mut frame).await.unwrap();
                    let request: Value = serde_json::from_str(&frame).unwrap();
                    reader
                        .get_mut()
                        .write_all(
                            format!(
                                "{}\n",
                                json!({"id":request["id"],"status":200,"body":{"ok":result}})
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                }
            });
            assert_eq!(
                readiness(&connection).await,
                if code == 200 {
                    RuntimeStatus::Ready
                } else {
                    RuntimeStatus::Unavailable
                }
            );
            server.await.unwrap();
        }
    }
}
