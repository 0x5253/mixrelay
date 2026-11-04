//! Runs each native command the background controller serves.
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tauri::Manager as _;

use super::command::NativeCommand;

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

fn unsupported() -> Value {
    Value::String("unsupported background command".into())
}

/// Local clients reach only the engine's versioned API, without a query or fragment.
fn engine_route(method: &str, path: &str) -> Result<reqwest::Method, Value> {
    if !matches!(method, "GET" | "PUT" | "POST")
        || !path.starts_with("/api/v1/")
        || path.contains(['?', '#', '\n', '\r'])
    {
        return Err(Value::String("invalid engine route".into()));
    }
    reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| Value::String("invalid engine method".into()))
}

// Keep the closed command boundary explicit so native authority can be audited in one place.
#[allow(clippy::too_many_lines)]
pub(crate) async fn dispatch(
    app: &tauri::AppHandle,
    name: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let Some(command) = NativeCommand::parse(name) else {
        return Err(unsupported());
    };
    match command {
        NativeCommand::Midi => app
            .state::<crate::midi::Midi>()
            .command(name, &arguments)
            .await
            .map_err(|error| Value::String(error.to_string())),
        NativeCommand::Hello => Ok(
            serde_json::json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()}),
        ),
        NativeCommand::SetNativeMenuLocale => {
            let locale: String = argument(&arguments, "locale")?;
            response(crate::app_menu::set_native_menu_locale(&locale, app.clone()).await)
        }
        NativeCommand::EngineRequest => {
            let method: String = argument(&arguments, "method")?;
            let path: String = argument(&arguments, "path")?;
            let method = engine_route(&method, &path)?;
            response(
                app.state::<crate::control_proxy::ControlProxyClient>()
                    .request(method, &path, argument(&arguments, "body")?)
                    .await,
            )
        }
        NativeCommand::WorkspacePath => response(
            app.state::<crate::control_proxy::workspace::WorkspaceFiles>()
                .current(),
        ),
        NativeCommand::WorkspaceSubmit => response(
            crate::control_proxy::workspace::submit(
                app,
                &app.state(),
                argument(&arguments, "action")?,
                argument(&arguments, "payload")?,
            )
            .await,
        ),
        NativeCommand::GetGraphLayout => response(
            crate::control_proxy::control_get_graph_layout(
                app.state(),
                argument(&arguments, "graphId")?,
            )
            .await,
        ),
        NativeCommand::SetGraphLayout => response(
            crate::control_proxy::control_set_graph_layout(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "request")?,
            )
            .await,
        ),
        NativeCommand::GetStatus => {
            response(crate::control_proxy::control_get_status(app.state(), app.clone()).await)
        }
        NativeCommand::ReconfigureSession => response(
            crate::control_proxy::control_reconfigure_session(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        NativeCommand::SetEnginePower => response(
            crate::control_proxy::control_set_engine_power(
                app.state(),
                app.clone(),
                argument(&arguments, "enabled")?,
            )
            .await,
        ),
        #[cfg(target_os = "linux")]
        NativeCommand::GetGraph => {
            response(crate::control_proxy::control_get_graph(app.state()).await)
        }
        NativeCommand::SetVirtualEndpointSampleRate => response(
            crate::control_proxy::control_set_virtual_endpoint_sample_rate(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        #[cfg(target_os = "macos")]
        NativeCommand::GetGraph => {
            response(crate::control_proxy::control_get_graph(app.state(), app.state()).await)
        }
        NativeCommand::GetOperation => response(
            crate::control_proxy::control_get_operation(
                app.state(),
                argument(&arguments, "operationId")?,
            )
            .await,
        ),
        NativeCommand::GetGraphStatus => {
            response(crate::control_proxy::control_get_graph_status(app.state()).await)
        }
        #[cfg(target_os = "linux")]
        NativeCommand::GetGraphCatalog => {
            response(crate::control_proxy::control_get_graph_catalog(app.state()).await)
        }
        #[cfg(target_os = "macos")]
        NativeCommand::GetGraphCatalog => response(
            crate::control_proxy::control_get_graph_catalog(app.state(), app.state()).await,
        ),
        #[cfg(target_os = "linux")]
        NativeCommand::MutateGraphCatalog => response(
            crate::control_proxy::control_mutate_graph_catalog(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        #[cfg(target_os = "macos")]
        NativeCommand::MutateGraphCatalog => response(
            crate::control_proxy::control_mutate_graph_catalog(
                app.state(),
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        NativeCommand::GetMeters => {
            response(crate::control_proxy::control_get_meters(app.state()).await)
        }
        NativeCommand::GetProcessors => {
            response(crate::control_proxy::control_get_processors(app.state()).await)
        }
        NativeCommand::RetryProcessor => response(
            crate::control_proxy::control_retry_processor(
                app.state(),
                argument(&arguments, "request")?,
            )
            .await,
        ),
        NativeCommand::GetProcessorEditor => response(
            crate::control_proxy::control_get_processor_editor(
                app.state(),
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        NativeCommand::RescanProcessors => response(
            crate::control_proxy::control_rescan_processors(
                app.state(),
                argument(&arguments, "paths")?,
            )
            .await,
        ),
        NativeCommand::SetProcessorParameter => response(
            crate::control_proxy::control_set_processor_parameter(
                app.state(),
                argument(&arguments, "nodeId")?,
                argument(&arguments, "parameterId")?,
                argument(&arguments, "value")?,
                argument(&arguments, "expectedRevision")?,
            )
            .await,
        ),
        NativeCommand::SetProcessorEditor => response(
            crate::control_proxy::control_set_processor_editor(
                app.state(),
                argument(&arguments, "nodeId")?,
                argument(&arguments, "open")?,
                argument(&arguments, "expectedRevision")?,
            )
            .await,
        ),
        NativeCommand::GetProcessorEditorFrame => response(
            crate::control_proxy::control_get_processor_editor_frame(
                app.state(),
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        NativeCommand::SendProcessorEditorInput => response(
            crate::control_proxy::control_send_processor_editor_input(
                app.state(),
                argument(&arguments, "nodeId")?,
                argument(&arguments, "input")?,
            )
            .await,
        ),
        #[cfg(target_os = "linux")]
        NativeCommand::ApplyGraph => response(
            crate::control_proxy::control_apply_graph(
                app.state(),
                argument(&arguments, "document")?,
            )
            .await,
        ),
        #[cfg(target_os = "macos")]
        NativeCommand::ApplyGraph => response(
            crate::control_proxy::control_apply_graph(
                app.state(),
                app.state(),
                argument(&arguments, "document")?,
            )
            .await,
        ),
        NativeCommand::GetNodeControls => response(
            crate::control_proxy::node_control::control_get_node_controls(
                app.state(),
                argument(&arguments, "graphId")?,
            )
            .await,
        ),
        NativeCommand::UpdateNodeControl => response(
            crate::control_proxy::node_control::control_update_node_control(
                app.state(),
                app.clone(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
                argument(&arguments, "request")?,
            )
            .await,
        ),
        NativeCommand::GetRecorderStatus => response(
            crate::control_proxy::recorder::control_get_recorder_status(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        NativeCommand::SetRecorder => response(
            crate::control_proxy::recorder::control_set_recorder(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
                argument(&arguments, "request")?,
            )
            .await,
        ),
        NativeCommand::RecordingSettings => response(
            crate::control_proxy::recording_settings::recording_settings(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
            )
            .await,
        ),
        NativeCommand::SaveRecordingSettings => response(
            crate::control_proxy::recording_settings::save_recording_settings(
                app.state(),
                argument(&arguments, "graphId")?,
                argument(&arguments, "nodeId")?,
                argument(&arguments, "settings")?,
                argument(&arguments, "expectedRevision")?,
            )
            .await,
        ),
        NativeCommand::DriverStatus => response(crate::driver_control::driver_status().await),
        NativeCommand::DriverMutate => response(
            crate::driver_control::driver_mutate(app.clone(), argument(&arguments, "command")?)
                .await,
        ),
        #[cfg(target_os = "macos")]
        NativeCommand::ListCapturableApplications => {
            response(crate::macos_audio::list_capturable_applications())
        }
        #[cfg(target_os = "macos")]
        NativeCommand::ListAudioDevices => response(crate::macos_audio::list_audio_devices()),
        #[cfg(target_os = "macos")]
        NativeCommand::StartCaptureProbe => response(crate::macos_audio::start_capture_probe(
            argument(&arguments, "bundleId")?,
            argument(&arguments, "muteWhileCaptured")?,
            app.state(),
        )),
        #[cfg(target_os = "macos")]
        NativeCommand::CaptureProbeStats => {
            response(crate::macos_audio::capture_probe_stats(app.state()))
        }
        #[cfg(target_os = "macos")]
        NativeCommand::StopCaptureProbe => {
            response(crate::macos_audio::stop_capture_probe(app.state()))
        }
        #[cfg(target_os = "linux")]
        NativeCommand::ListCapturableApplications => {
            let discovery = std::sync::Arc::clone(
                app.state::<std::sync::Arc<crate::linux_audio::ApplicationDiscovery>>()
                    .inner(),
            );
            discover(move || discovery.applications().map_err(|error| error.to_string())).await
        }
        #[cfg(target_os = "linux")]
        NativeCommand::ListAudioDevices => discover(crate::linux_audio::list_audio_devices).await,
        #[cfg(not(target_os = "macos"))]
        NativeCommand::StartCaptureProbe
        | NativeCommand::CaptureProbeStats
        | NativeCommand::StopCaptureProbe => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests;
