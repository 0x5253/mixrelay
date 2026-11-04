//! The desktop application: the graph editor and its background Driver runtime share one
//! executable and one signed bundle.
use std::{ffi::OsString, sync::Arc};

use app_mode::AppMode;

mod app_events;
mod app_menu;
mod app_mode;
mod async_time;
mod background;
mod commands;
mod control_channel;
mod control_proxy;
mod diagnostics;
mod driver_control;
mod engine_exit;
#[cfg(target_os = "linux")]
mod engine_supervision;
mod logging;
mod midi;
mod native_text;
mod preferences_file;
mod processor_dialog;
mod recording_dialog;
mod setup;
mod status_menu;
mod system_stop;
mod updates;
mod workspace_menu;

#[cfg(target_os = "linux")]
mod child_engine_lifecycle;
#[cfg(target_os = "macos")]
mod engine_lifecycle;
#[cfg(target_os = "linux")]
use child_engine_lifecycle as engine_lifecycle;
#[cfg(any(test, target_os = "macos"))]
mod macos_webview;
#[cfg(target_os = "macos")]
mod native_termination;

#[cfg(target_os = "macos")]
use mixrelay_macos_transport as _;

#[cfg(target_os = "linux")]
mod linux_audio;
#[cfg(any(test, target_os = "linux"))]
mod linux_service;
#[cfg(target_os = "macos")]
mod macos_audio;

#[cfg(target_os = "linux")]
use linux_audio as platform_audio;
#[cfg(target_os = "macos")]
use macos_audio as platform_audio;

type Builder = tauri::Builder<tauri::Wry>;

/// Runs the editor, or the background Driver runtime when `--driver-runtime` is given, until
/// the application exits.
pub fn run(arguments: &[OsString]) {
    let mode = AppMode::detect(arguments.iter().map(OsString::as_os_str));
    logging::init(mode.log_role());
    let mut context = tauri::generate_context!();
    mode.configure(context.config_mut());
    let builder = manage_platform_state(Builder::default().manage(mode), context.config_mut());
    #[cfg(target_os = "macos")]
    let builder = admit_runtime(builder, mode);
    let builder = admit_single_editor(builder, mode);
    let builder = commands::register(manage_control_state(builder))
        .setup(setup::setup_desktop)
        .on_window_event(app_events::on_window_event);
    match builder.build(context) {
        Ok(application) => {
            #[cfg(target_os = "macos")]
            let application = keep_runtime_out_of_dock(application, mode);
            application.run(|app, event| app_events::on_run_event(app, &event));
        }
        Err(error) => {
            #[cfg(target_os = "macos")]
            native_termination::release();
            tracing::error!(%error, "could not initialize MixRelay desktop");
        }
    }
}

#[cfg(target_os = "linux")]
fn manage_platform_state(builder: Builder, _config: &mut tauri::Config) -> Builder {
    builder.manage(Arc::new(linux_audio::ApplicationDiscovery::new(
        mixrelay_time::StdClock::new(),
    )))
}

#[cfg(target_os = "macos")]
fn manage_platform_state(builder: Builder, config: &mut tauri::Config) -> Builder {
    builder.manage(macos_webview::prepare(config))
}

/// Ownership contention is an expected rejected launch. Tauri panics on setup-hook errors,
/// so the runtime is admitted before its event loop starts.
#[cfg(target_os = "macos")]
fn admit_runtime(builder: Builder, mode: AppMode) -> Builder {
    if mode != AppMode::Service {
        return builder;
    }
    match engine_lifecycle::EngineLifecycle::start() {
        Ok(lifecycle) => builder.manage(lifecycle),
        Err(error) => {
            tracing::error!(%error, "Driver runtime startup was not admitted");
            std::process::exit(1);
        }
    }
}

/// A second editor launch shows the running editor instead.
fn admit_single_editor(builder: Builder, mode: AppMode) -> Builder {
    if mode != AppMode::Graph {
        return builder;
    }
    builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
        app_events::reopen_editor(app);
    }))
}

/// State behind the control commands, in both modes.
fn manage_control_state(builder: Builder) -> Builder {
    let builder = builder
        .manage(Arc::new(
            mixrelay_desktop_core::diagnostics::ExportAdmission::default(),
        ))
        .manage(control_proxy::ControlProxyClient::for_session())
        .manage(control_proxy::workspace::WorkspaceFiles::default())
        .manage(control_proxy::telemetry::TelemetrySubscriptions::default());
    #[cfg(target_os = "macos")]
    let builder = builder
        .manage(macos_audio::CaptureProbeState::default())
        .manage(Arc::new(macos_audio::ApplicationCaptureState::default()));
    builder
}

/// Setup runs after Tao applies its initial activation policy at launch, so the runtime
/// leaves the Dock only once the application is built.
#[cfg(target_os = "macos")]
fn keep_runtime_out_of_dock(mut application: tauri::App, mode: AppMode) -> tauri::App {
    if mode == AppMode::Service {
        application.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    application
}
