//! The webview's Tauri commands. In the editor, those the background controller serves are
//! forwarded to it before any local handler runs.
#[cfg(target_os = "macos")]
use crate::macos_audio;
use crate::{
    app_menu, background, control_proxy, diagnostics, driver_control, logging, platform_audio,
    processor_dialog, recording_dialog, updates, workspace_menu,
};

pub(crate) fn register(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    let local: Box<dyn Fn(tauri::ipc::Invoke) -> bool + Send + Sync> =
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
        Some(invoke) => local(invoke),
    })
}
