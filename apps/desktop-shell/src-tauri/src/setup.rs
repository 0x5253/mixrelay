//! Tauri setup: the editor connects to the background runtime; the runtime owns the engine,
//! local controls and the status menu.
use std::error::Error;

use tauri::Manager;

use crate::{
    app_mode::AppMode, background, control_proxy, engine_lifecycle, midi, status_menu, system_stop,
    workspace_menu,
};
#[cfg(target_os = "macos")]
use crate::{macos_audio, macos_webview, native_termination};

pub(crate) fn setup_desktop(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    app.manage(native_termination::install(app.handle())?);
    #[cfg(target_os = "linux")]
    system_stop::observe_background_stop(app.handle())?;
    let mode = *app.state::<AppMode>();
    match mode {
        AppMode::Graph => setup_editor(app),
        AppMode::Service => setup_runtime(app),
    }
}

fn setup_editor(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    app.manage(macos_audio::DeviceActivityCache::new());
    #[cfg(target_os = "macos")]
    macos_webview::create_deferred(app)?;
    connect_to_runtime(app.handle())?;
    workspace_menu::install(app)?;
    if tauri::is_dev()
        && let Some(window) = app.get_webview_window("main")
    {
        window.set_title("MixRelay (Dev)")?;
    }
    Ok(())
}

fn setup_runtime(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    control_proxy::recording_settings::initialize(app)?;
    // Publish control identity only after lifecycle ownership is admitted.
    // A duplicate cold launch must never bind ahead of the real controller.
    own_engine(app.handle())?;
    install_local_controls(app.handle())?;
    status_menu::install(app)?;
    #[cfg(target_os = "macos")]
    system_stop::observe_background_stop(app.handle())?;
    background::restore_driver(app.handle());
    background::restore_captures(app.handle());
    Ok(())
}

/// The editor's one runtime connection, shared with its control proxy.
fn connect_to_runtime(app: &tauri::AppHandle) -> Result<(), AudioStartupFailure> {
    if app.try_state::<background::Connection>().is_some() {
        return Ok(());
    }
    let connection = background::Connection::for_app(app)?;
    *app.state::<control_proxy::ControlProxyClient>()
        .background
        .lock()
        .map_err(|_| AudioStartupFailure::ConnectionLock)? = Some(connection.clone());
    app.manage(connection);
    Ok(())
}

/// The macOS runtime was admitted before its event loop; elsewhere the engine starts here.
fn own_engine(app: &tauri::AppHandle) -> Result<(), AudioStartupFailure> {
    if app
        .try_state::<engine_lifecycle::EngineLifecycle>()
        .is_none()
    {
        app.manage(
            engine_lifecycle::EngineLifecycle::start().map_err(AudioStartupFailure::Engine)?,
        );
    }
    engine_lifecycle::monitor(app);
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum AudioStartupFailure {
    #[error(transparent)]
    Connection(#[from] background::ConnectionFailure),
    #[error("background connection lock poisoned")]
    ConnectionLock,
    #[error("engine startup failed: {0}")]
    Engine(String),
}

fn install_local_controls(app: &tauri::AppHandle) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::MetadataExt;
    let directory = background::control_directory(app)?;
    let uid = std::fs::metadata(app.path().home_dir()?)?.uid();
    let control = control_proxy::local::LocalControl::new(
        app.state::<control_proxy::ControlProxyClient>()
            .inner()
            .clone(),
        #[cfg(target_os = "macos")]
        std::sync::Arc::clone(
            app.state::<std::sync::Arc<macos_audio::ApplicationCaptureState>>()
                .inner(),
        ),
    )?
    .with_app(app.clone());
    let control = std::sync::Arc::new(control);
    let midi = midi::Midi::start(app, std::sync::Arc::clone(&control))?;
    app.manage(midi);
    let socket = tauri::async_runtime::block_on(control_proxy::local::socket::LocalSocket::start(
        &directory, uid, control,
    ))?;
    app.manage(socket);
    Ok(())
}
