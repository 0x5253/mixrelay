//! The desktop application: the graph editor and its background Driver runtime share one
//! executable and one signed bundle.
use std::ffi::OsString;

use tauri::{Manager, WindowEvent};

mod app_menu;
mod app_mode;
mod async_time;
mod background;
mod control_channel;
mod control_proxy;
mod diagnostics;
mod driver_control;
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

/// Runs the editor, or the background Driver runtime when `--driver-runtime` is given, until
/// the application exits.
pub fn run(arguments: &[OsString]) {
    let mode = app_mode::AppMode::detect(arguments.iter().map(OsString::as_os_str));
    logging::init(match mode {
        app_mode::AppMode::Service => "driver-runtime",
        app_mode::AppMode::Graph => "desktop",
    });
    let mut context = tauri::generate_context!();
    mode.configure(context.config_mut());
    let builder = tauri::Builder::default().manage(mode);
    #[cfg(target_os = "linux")]
    let builder = builder.manage(std::sync::Arc::new(linux_audio::ApplicationDiscovery::new(
        mixrelay_time::StdClock::new(),
    )));
    #[cfg(target_os = "macos")]
    let builder = builder.manage(macos_webview::prepare(context.config_mut()));
    #[cfg(target_os = "macos")]
    let builder = if mode == app_mode::AppMode::Service {
        // Ownership contention is an expected rejected launch. Tauri panics on
        // setup-hook errors, so admit the controller before starting its loop.
        match engine_lifecycle::EngineLifecycle::start() {
            Ok(lifecycle) => builder.manage(lifecycle),
            Err(error) => {
                tracing::error!(%error, "Driver runtime startup was not admitted");
                std::process::exit(1);
            }
        }
    } else {
        builder
    };
    let builder = if mode == app_mode::AppMode::Graph {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            background::reopen_editor(app);
        }))
    } else {
        builder
    };
    let application = graph_builder(builder)
        .setup(setup::setup_desktop)
        .on_window_event(handle_window_event)
        .build(context);
    let application = match application {
        Ok(application) => application,
        Err(error) => {
            #[cfg(target_os = "macos")]
            native_termination::release();
            tracing::error!(%error, "could not initialize MixRelay desktop");
            return;
        }
    };
    #[cfg(target_os = "macos")]
    let mut application = application;
    #[cfg(target_os = "macos")]
    if mode == app_mode::AppMode::Service {
        // Setup runs after Tao applies its initial policy at application launch.
        application.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    application.run(|app, event| handle_application_event(app, &event));
}

fn graph_builder(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    let builder = builder
        .manage(std::sync::Arc::new(diagnostics::ExportAdmission::default()))
        .manage(control_proxy::ControlProxyClient::for_session())
        .manage(control_proxy::workspace::WorkspaceFiles::default())
        .manage(control_proxy::telemetry::TelemetrySubscriptions::default());
    #[cfg(target_os = "macos")]
    let builder = builder
        .manage(macos_audio::CaptureProbeState::default())
        .manage(std::sync::Arc::new(
            macos_audio::ApplicationCaptureState::default(),
        ));
    let handler: Box<dyn Fn(tauri::ipc::Invoke) -> bool + Send + Sync> =
        Box::new(tauri::generate_handler![
            logging::webview_error,
            diagnostics::diagnostics_save_report,
            background::startup::runtime_connect,
            background::startup::runtime_status,
            driver_control::driver_status,
            driver_control::driver_mutate,
            updates::updates_check,
            updates::updates_open_download,
            control_proxy::node_control::control_get_node_controls,
            control_proxy::node_control::control_update_node_control,
            app_menu::set_native_menu_locale,
            workspace_menu::set_graph_list_visible,
            control_proxy::control_get_status,
            control_proxy::workspace::workspace_file,
            control_proxy::control_reconfigure_session,
            control_proxy::control_set_engine_power,
            control_proxy::control_set_virtual_endpoint_sample_rate,
            control_proxy::control_get_graph,
            control_proxy::control_get_graph_catalog,
            control_proxy::control_get_graph_layout,
            control_proxy::control_set_graph_layout,
            control_proxy::control_mutate_graph_catalog,
            control_proxy::control_get_graph_status,
            control_proxy::control_get_operation,
            control_proxy::control_get_meters,
            control_proxy::telemetry::control_subscribe_telemetry,
            control_proxy::telemetry::control_telemetry_credit,
            control_proxy::telemetry::control_telemetry_visibility,
            control_proxy::telemetry::control_unsubscribe_telemetry,
            control_proxy::control_get_processors,
            control_proxy::control_retry_processor,
            control_proxy::control_get_processor_editor,
            control_proxy::control_get_processor_editor_frame,
            control_proxy::control_send_processor_editor_input,
            control_proxy::control_rescan_processors,
            control_proxy::control_set_processor_parameter,
            control_proxy::control_set_processor_editor,
            control_proxy::control_apply_graph,
            platform_audio::list_capturable_applications,
            platform_audio::list_device_app_activity,
            platform_audio::list_audio_devices,
            #[cfg(target_os = "macos")]
            macos_audio::start_capture_probe,
            #[cfg(target_os = "macos")]
            macos_audio::capture_probe_stats,
            #[cfg(target_os = "macos")]
            macos_audio::stop_capture_probe,
            processor_dialog::select_processor_directory,
            recording_dialog::select_recording_folder,
            control_proxy::recording_settings::recording_settings,
            control_proxy::recording_settings::save_recording_settings,
            control_proxy::recorder::control_get_recorder_status,
            control_proxy::recorder::control_set_recorder,
        ]);
    builder.invoke_handler(move |invoke| match background::forward(invoke) {
        None => true,
        Some(invoke) => handler(invoke),
    })
}

fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event
        && window.label() == "main"
    {
        api.prevent_close();
        let app = window.app_handle().clone();
        tauri::async_runtime::spawn(async move {
            midi::cancel_editor_learn(&app).await;
            #[cfg(target_os = "linux")]
            if *app.state::<app_mode::AppMode>() == app_mode::AppMode::Graph {
                app.exit(0);
                return;
            }
            if let Err(error) = app_menu::hide_main_window(&app) {
                tracing::error!(%error, "could not hide main window");
            }
        });
    }
}

fn handle_application_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    #[cfg(target_os = "macos")]
    if matches!(event, tauri::RunEvent::Exit) {
        native_termination::release();
    }
    if let tauri::RunEvent::ExitRequested { api, .. } = event
        && let Some(lifecycle) = app.try_state::<engine_lifecycle::EngineLifecycle>()
    {
        lifecycle.exit_requested(app, api);
    }
    #[cfg(target_os = "macos")]
    if let tauri::RunEvent::Reopen { .. } = event
        && *app.state::<app_mode::AppMode>() == app_mode::AppMode::Service
    {
        // With no editor running, reopening MixRelay targets this process, and
        // LaunchServices promotes it to a Dock app because the shared bundle is a
        // regular app. The runtime stays in the background; the editor it opens is
        // the one Dock app.
        if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
            tracing::error!(%error, "could not keep the runtime in the background");
        }
        status_menu::open_editor();
    }
    #[cfg(target_os = "macos")]
    if let tauri::RunEvent::Reopen { .. } = event
        && *app.state::<app_mode::AppMode>() == app_mode::AppMode::Graph
    {
        background::reopen_editor(app);
    }
}
