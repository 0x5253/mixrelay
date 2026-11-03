//! Closed native command set served by the background controller.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tauri::Manager as _;

pub(crate) fn is_service_command(name: &str) -> bool {
    matches!(
        name,
        "midi_map"
            | "midi_status"
            | "midi_learn"
            | "midi_cancel"
            | "midi_save"
            | "midi_remove"
            | "control_get_graph_layout"
            | "control_set_graph_layout"
            | "control_get_status"
            | "control_reconfigure_session"
            | "control_set_engine_power"
            | "control_get_graph"
            | "control_set_virtual_endpoint_sample_rate"
            | "control_get_graph_status"
            | "control_get_operation"
            | "control_get_graph_catalog"
            | "control_mutate_graph_catalog"
            | "control_get_meters"
            | "control_get_processors"
            | "control_retry_processor"
            | "control_get_processor_editor"
            | "control_rescan_processors"
            | "control_set_processor_parameter"
            | "control_set_processor_editor"
            | "control_get_processor_editor_frame"
            | "control_send_processor_editor_input"
            | "control_apply_graph"
            | "control_get_node_controls"
            | "control_update_node_control"
            | "control_get_recorder_status"
            | "control_set_recorder"
            | "recording_settings"
            | "save_recording_settings"
            | "driver_status"
            | "driver_mutate"
            | "list_capturable_applications"
            | "list_audio_devices"
            | "start_capture_probe"
            | "capture_probe_stats"
            | "stop_capture_probe"
    )
}

fn argument<T: DeserializeOwned>(arguments: &Value, name: &str) -> Result<T, Value> {
    serde_json::from_value(arguments.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|_| Value::String(format!("invalid or missing argument: {name}")))
}

fn response<T: Serialize, E: Into<tauri::ipc::InvokeError>>(
    result: Result<T, E>,
) -> Result<Value, Value> {
    result.map_err(|error| error.into().0).and_then(|value| {
        serde_json::to_value(value).map_err(|error| Value::String(error.to_string()))
    })
}

#[cfg(any(target_os = "linux", test))]
async fn discover<T, E>(
    query: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<Value, Value>
where
    T: Serialize + Send + 'static,
    E: Into<tauri::ipc::InvokeError> + Send + 'static,
{
    // PipeWire discovery owns bounded child processes. The caller awaits their
    // completion while the async runtime continues serving commands and liveness.
    tokio::task::spawn_blocking(move || response(query()))
        .await
        .map_err(|error| Value::String(error.to_string()))?
}

// Keep the closed command boundary explicit so native authority can be audited in one place.
#[allow(clippy::too_many_lines)]
pub(crate) async fn dispatch(
    app: &tauri::AppHandle,
    name: &str,
    arguments: Value,
) -> Result<Value, Value> {
    match name {
        "midi_map" | "midi_status" | "midi_learn" | "midi_cancel" | "midi_save" | "midi_remove" => {
            app.state::<crate::midi::Midi>()
                .command(name, &arguments)
                .await
                .map_err(|error| Value::String(error.to_string()))
        }
        "hello" => Ok(
            serde_json::json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()}),
        ),
        "set_native_menu_locale" => {
            let locale: String = argument(&arguments, "locale")?;
            response(crate::app_menu::set_native_menu_locale(&locale, app.clone()).await)
        }
        "engine_request" => {
            let method: String = argument(&arguments, "method")?;
            let path: String = argument(&arguments, "path")?;
            if !matches!(method.as_str(), "GET" | "PUT" | "POST")
                || !path.starts_with("/api/v1/")
                || path.contains(['?', '#', '\n', '\r'])
            {
                return Err(Value::String("invalid engine route".into()));
            }
            let method = reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|_| Value::String("invalid engine method".into()))?;
            response(
                app.state::<crate::control_proxy::ControlProxyClient>()
                    .request(method, &path, argument(&arguments, "body")?)
                    .await,
            )
        }
        "workspace_path" => response(
            app.state::<crate::control_proxy::workspace::WorkspaceFiles>()
                .current(),
        ),
        "workspace_submit" => response(
            crate::control_proxy::workspace::submit(
                app,
                &app.state(),
                argument(&arguments, "action")?,
                argument(&arguments, "payload")?,
            )
            .await,
        ),
        "control_get_graph_layout" => response(
            crate::control_proxy::control_get_graph_layout(
                app.state(),
                argument(&arguments, "graphId")?,
            )
            .await,
        ),
        "control_set_graph_layout" => response(
            crate::control_proxy::control_set_graph_layout(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "request")?,
            )
            .await,
        ),
        "control_get_status" => {
            response(crate::control_proxy::control_get_status(app.state(), app.clone()).await)
        }
        "control_reconfigure_session" => response(
            crate::control_proxy::control_reconfigure_session(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        "control_set_engine_power" => response(
            crate::control_proxy::control_set_engine_power(
                app.state(),
                app.clone(),
                argument(&arguments, "enabled")?,
            )
            .await,
        ),
        #[cfg(target_os = "linux")]
        "control_get_graph" => response(crate::control_proxy::control_get_graph(app.state()).await),
        "control_set_virtual_endpoint_sample_rate" => response(
            crate::control_proxy::control_set_virtual_endpoint_sample_rate(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        #[cfg(target_os = "macos")]
        "control_get_graph" => {
            response(crate::control_proxy::control_get_graph(app.state(), app.state()).await)
        }
        "control_get_operation" => response(
            crate::control_proxy::control_get_operation(
                app.state(),
                argument(&arguments, "operationId")?,
            )
            .await,
        ),
        "control_get_graph_status" => {
            response(crate::control_proxy::control_get_graph_status(app.state()).await)
        }
        #[cfg(target_os = "linux")]
        "control_get_graph_catalog" => {
            response(crate::control_proxy::control_get_graph_catalog(app.state()).await)
        }
        #[cfg(target_os = "macos")]
        "control_get_graph_catalog" => response(
            crate::control_proxy::control_get_graph_catalog(app.state(), app.state()).await,
        ),
        #[cfg(target_os = "linux")]
        "control_mutate_graph_catalog" => response(
            crate::control_proxy::control_mutate_graph_catalog(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        #[cfg(target_os = "macos")]
        "control_mutate_graph_catalog" => response(
            crate::control_proxy::control_mutate_graph_catalog(
                app.state(),
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        "control_get_meters" => {
            response(crate::control_proxy::control_get_meters(app.state()).await)
        }
        "control_get_processors" => {
            response(crate::control_proxy::control_get_processors(app.state()).await)
        }
        "control_retry_processor" => response(
            crate::control_proxy::control_retry_processor(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        "control_get_processor_editor" => response(
            crate::control_proxy::control_get_processor_editor(
                app.state(),
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        "control_rescan_processors" => response(
            crate::control_proxy::control_rescan_processors(
                app.state(),
                argument(&arguments, "paths")?,
            )
            .await,
        ),
        "control_set_processor_parameter" => response(
            crate::control_proxy::control_set_processor_parameter(
                app.state(),
                argument(&arguments, "nodeId")?,
                argument(&arguments, "parameterId")?,
                argument(&arguments, "value")?,
                argument(&arguments, "expectedRevision")?,
            )
            .await,
        ),
        "control_set_processor_editor" => response(
            crate::control_proxy::control_set_processor_editor(
                app.state(),
                argument(&arguments, "nodeId")?,
                argument(&arguments, "open")?,
                argument(&arguments, "expectedRevision")?,
            )
            .await,
        ),
        "control_get_processor_editor_frame" => response(
            crate::control_proxy::control_get_processor_editor_frame(
                app.state(),
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        "control_send_processor_editor_input" => response(
            crate::control_proxy::control_send_processor_editor_input(
                app.state(),
                argument(&arguments, "nodeId")?,
                argument(&arguments, "input")?,
            )
            .await,
        ),
        #[cfg(target_os = "linux")]
        "control_apply_graph" => response(
            crate::control_proxy::control_apply_graph(
                app.state(),
                argument(&arguments, "document")?,
            )
            .await,
        ),
        #[cfg(target_os = "macos")]
        "control_apply_graph" => response(
            crate::control_proxy::control_apply_graph(
                app.state(),
                app.state(),
                argument(&arguments, "document")?,
            )
            .await,
        ),
        "control_get_node_controls" => response(
            crate::control_proxy::node_control::control_get_node_controls(
                app.state(),
                argument(&arguments, "graphId")?,
            )
            .await,
        ),
        "control_update_node_control" => response(
            crate::control_proxy::node_control::control_update_node_control(
                app.state(),
                app.clone(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
                argument(&arguments, "request")?,
            )
            .await,
        ),
        "control_get_recorder_status" => response(
            crate::control_proxy::recorder::control_get_recorder_status(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        "control_set_recorder" => response(
            crate::control_proxy::recorder::control_set_recorder(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
                argument(&arguments, "request")?,
            )
            .await,
        ),
        "recording_settings" => response(
            crate::control_proxy::recording_settings::recording_settings(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        "save_recording_settings" => response(
            crate::control_proxy::recording_settings::save_recording_settings(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
                argument(&arguments, "settings")?,
                argument(&arguments, "expectedRevision")?,
            )
            .await,
        ),
        "driver_status" => response(crate::driver_control::driver_status().await),
        "driver_mutate" => response(
            crate::driver_control::driver_mutate(app.clone(), argument(&arguments, "command")?)
                .await,
        ),
        #[cfg(target_os = "macos")]
        "list_capturable_applications" => {
            response(crate::macos_audio::list_capturable_applications())
        }
        #[cfg(target_os = "macos")]
        "list_audio_devices" => response(crate::macos_audio::list_audio_devices()),
        #[cfg(target_os = "macos")]
        "start_capture_probe" => response(crate::macos_audio::start_capture_probe(
            argument(&arguments, "bundleId")?,
            argument(&arguments, "muteWhileCaptured")?,
            app.state(),
        )),
        #[cfg(target_os = "macos")]
        "capture_probe_stats" => response(crate::macos_audio::capture_probe_stats(app.state())),
        #[cfg(target_os = "macos")]
        "stop_capture_probe" => response(crate::macos_audio::stop_capture_probe(app.state())),
        #[cfg(target_os = "linux")]
        "list_capturable_applications" => {
            let discovery = std::sync::Arc::clone(
                app.state::<std::sync::Arc<crate::linux_audio::ApplicationDiscovery>>()
                    .inner(),
            );
            discover(move || discovery.applications().map_err(|error| error.to_string())).await
        }
        #[cfg(target_os = "linux")]
        "list_audio_devices" => discover(crate::linux_audio::list_audio_devices).await,
        _ => Err(Value::String("unsupported background command".into())),
    }
}

#[cfg(test)]
mod tests;
