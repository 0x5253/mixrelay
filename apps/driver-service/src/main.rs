//! Installed Driver control owner. Explicit manager quit stops devices through its API;
//! the service remains reachable independently of manager and graph process lifetimes.

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Context, Result, bail};
use mixrelay_driver_adapters::{FileSettingsStore, SystemLatencyGenerations};
use mixrelay_driver_control::{DriverManager, DriverProcessId, DriverRuntime};
use mixrelay_local_socket::{
    AuthorizedListener, DRIVER_SOCKET, bind, current_uid, socket_directory,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if std::env::args().skip(1).eq(["--version"]) {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: syslog retains the static identifier, and the process owns this logger.
        unsafe {
            libc::openlog(
                c"io.mixrelay.audio.driver".as_ptr(),
                libc::LOG_PID,
                libc::LOG_DAEMON,
            );
        };
    }
    let subscriber = tracing_subscriber::fmt().with_ansi(false);
    #[cfg(target_os = "macos")]
    let subscriber = subscriber.with_writer(ServiceLog);
    #[cfg(target_os = "linux")]
    let subscriber = subscriber.with_writer(std::io::stderr);
    subscriber.init();
    tracing::info!("Driver service starting");
    if let Err(error) = run().await {
        let detail = format!("{error:#}");
        tracing::error!(error = %detail, "Driver service failed");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
struct ServiceLog;

#[cfg(target_os = "macos")]
struct ServiceLogWriter(i32);

#[cfg(target_os = "macos")]
impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for ServiceLog {
    type Writer = ServiceLogWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        ServiceLogWriter(libc::LOG_INFO)
    }

    fn make_writer_for(&'writer self, metadata: &tracing::Metadata<'_>) -> Self::Writer {
        let priority = match *metadata.level() {
            tracing::Level::ERROR => libc::LOG_ERR,
            tracing::Level::WARN => libc::LOG_WARNING,
            tracing::Level::INFO => libc::LOG_INFO,
            tracing::Level::DEBUG | tracing::Level::TRACE => libc::LOG_DEBUG,
        };
        ServiceLogWriter(priority)
    }
}

#[cfg(target_os = "macos")]
impl std::io::Write for ServiceLogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let message = std::ffi::CString::new(String::from_utf8_lossy(buffer).replace('\0', "\\0"))?;
        // SAFETY: the fixed format accepts exactly one live terminated string pointer.
        // syslog copies its text during the call; log text never becomes a format string.
        unsafe { libc::syslog(self.0, c"%s".as_ptr(), message.as_ptr()) };
        std::io::stderr().write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

fn driver_runtime() -> Result<DriverRuntime> {
    Ok(DriverRuntime::new(
        DriverProcessId::new(std::process::id())?,
        Box::new(mixrelay_time::StdClock::new()),
        Box::new(SystemLatencyGenerations),
    ))
}

async fn run() -> Result<()> {
    let path = settings_path(std::env::args_os().skip(1))?;
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
    let manager = tokio::task::spawn_blocking(move || open_manager(path))
        .await
        .context("initialize native Driver owner")??;
    tracing::info!("Driver control API starting; native readiness is reported separately");
    axum::serve(
        listener,
        mixrelay_driver_adapters::router(manager)
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

fn open_manager(path: PathBuf) -> Result<DriverManager> {
    #[cfg(target_os = "macos")]
    let backend = {
        let policy = transport_compatibility(std::env::var("MIXRELAY_TRANSPORT_COMPATIBILITY"))?;
        let policy = mixrelay_macos_transport::control_inventory::ProducerPolicy::new(&policy)
            .context("invalid transport compatibility policy")?;
        mixrelay_macos_driver::MacDriver::open(policy).context("open macOS Driver")?
    };
    #[cfg(target_os = "linux")]
    let backend = mixrelay_linux_driver::LinuxDriver::open().context("open PipeWire Driver")?;
    let mut manager = DriverManager::open(backend, FileSettingsStore::new(path), driver_runtime()?)
        .context("restore Driver settings")?;
    #[cfg(target_os = "macos")]
    manager.set_installed_version(installed_driver_version());
    #[cfg(target_os = "linux")]
    // The running service binary carries the Driver release, independently of the graph app.
    manager.set_installed_version(Some(env!("CARGO_PKG_VERSION").into()));
    Ok(manager)
}

#[cfg(any(target_os = "macos", test))]
fn transport_compatibility(value: Result<String, std::env::VarError>) -> Result<Vec<String>> {
    // Transport compatibility is configured in the service, outside HAL binaries.
    let value = match value {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) =>
            "com.tencent.tencentmeeting,net.whatsapp.WhatsApp,com.motu.DigitalPerformer,com.vmware.fusion,com.goodnotesapp.x".into(),
        Err(error) => return Err(error).context("read transport compatibility override"),
    };
    mixrelay_driver_control::parse_transport_compatibility(&value)
        .context("parse MIXRELAY_TRANSPORT_COMPATIBILITY")
}

#[cfg(target_os = "macos")]
fn installed_driver_version() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args([
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "-o",
            "-",
            "/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver/Contents/Info.plist",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!version.is_empty()).then_some(version)
}

fn settings_path(mut arguments: impl Iterator<Item = OsString>) -> Result<PathBuf> {
    if let Some(argument) = arguments.next() {
        if argument != "--settings" {
            bail!(
                "unknown Driver service argument: {}",
                argument.to_string_lossy()
            );
        }
        let path = arguments.next().context("--settings requires a path")?;
        if arguments.next().is_some() {
            bail!("unexpected Driver service arguments");
        }
        return Ok(path.into());
    }
    default_settings_path(
        std::env::var_os("HOME"),
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
    )
}

fn default_settings_path(
    home: Option<OsString>,
    xdg_config: Option<&std::ffi::OsStr>,
) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = xdg_config;
        let home = home.context("Driver service requires the installed user's HOME")?;
        Ok(PathBuf::from(home).join("Library/Application Support/io.mixrelay.driver/drivers.json"))
    }
    #[cfg(target_os = "linux")]
    {
        let directory = if let Some(directory) = xdg_config.filter(|path| !path.is_empty()) {
            PathBuf::from(directory)
        } else {
            PathBuf::from(home.context("Driver service requires HOME or XDG_CONFIG_HOME")?)
                .join(".config")
        };
        if !directory.is_absolute() {
            bail!("Driver configuration directory must be absolute");
        }
        Ok(directory.join("mixrelay-driver/drivers.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_policy_defaults_and_development_override() {
        assert_eq!(
            transport_compatibility(Err(std::env::VarError::NotPresent))
                .unwrap()
                .len(),
            5
        );
        assert!(
            transport_compatibility(Ok(String::new()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            transport_compatibility(Ok("com.test.App".into())).unwrap(),
            ["com.test.App"]
        );
        assert!(transport_compatibility(Ok("com.*".into())).is_err());
        assert!(
            transport_compatibility(Err(std::env::VarError::NotUnicode(OsString::from("bad"))))
                .is_err()
        );
    }

    #[test]
    fn explicit_settings_path_does_not_acquire_a_desktop_lease() {
        let path = settings_path(
            [
                OsString::from("--settings"),
                OsString::from("/owned/drivers.json"),
            ]
            .into_iter(),
        )
        .expect("valid settings path");
        assert_eq!(path, PathBuf::from("/owned/drivers.json"));
    }

    #[test]
    fn service_rejects_graph_desktop_ownership_arguments() {
        assert!(settings_path([OsString::from("--desktop-owned")].into_iter()).is_err());
        assert!(settings_path([OsString::from("--settings")].into_iter()).is_err());
        assert!(
            settings_path(
                [
                    OsString::from("--settings"),
                    OsString::from("/owned/drivers.json"),
                    OsString::from("extra")
                ]
                .into_iter()
            )
            .is_err()
        );
    }

    #[test]
    fn default_store_belongs_to_the_service_user() {
        let path = default_settings_path(Some(OsString::from("/Users/driver-user")), None)
            .expect("user directory");
        #[cfg(target_os = "macos")]
        assert_eq!(
            path,
            PathBuf::from(
                "/Users/driver-user/Library/Application Support/io.mixrelay.driver/drivers.json"
            )
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            path,
            PathBuf::from("/Users/driver-user/.config/mixrelay-driver/drivers.json")
        );
        assert!(default_settings_path(None, None).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xdg_store_uses_only_an_absolute_user_configuration_directory() {
        assert_eq!(
            default_settings_path(None, Some(std::ffi::OsStr::new("/user-config")))
                .expect("XDG directory"),
            PathBuf::from("/user-config/mixrelay-driver/drivers.json")
        );
        assert!(default_settings_path(None, Some(std::ffi::OsStr::new("relative"))).is_err());
    }
}
