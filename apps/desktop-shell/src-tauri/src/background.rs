//! One user-owned controller keeps audio alive independently of editor windows.
use serde_json::{Value, json};
use tauri::Manager as _;

mod connection;
mod connection_failure;
mod directories;
mod dispatch;
mod process;
use connection::EXCHANGE_TIMEOUT;
pub(crate) use connection::{Connection, StartedTelemetrySubscription};
pub(crate) use connection_failure::ConnectionFailure;
pub(crate) use directories::{control_directory, data_directory};
pub(crate) use process::{ControllerLaunch, spawn_controller, spawn_editor};
mod link;
pub(crate) mod startup;
pub(crate) use dispatch::{dispatch, is_service_command};

fn forward_transport_error(name: &str, error: &ConnectionFailure) -> Result<Value, Value> {
    let diagnostic = error.diagnostic();
    tracing::warn!(
        command = name,
        platform = ?diagnostic.platform,
        boundary = ?diagnostic.boundary,
        stage = ?diagnostic.stage,
        failure = ?diagnostic.failure,
        delivery = ?diagnostic.delivery,
        os_error = diagnostic.os_error,
        "background command transport failed"
    );
    if name == "control_get_graph_catalog" && error.catalog_read_retryable() {
        Ok(
            json!({"status":503,"body":{"code":"control_unavailable","parameters":{
                "reason": error.to_string(), "diagnostic": diagnostic
            }}}),
        )
    } else {
        Err(Value::String(error.to_string()))
    }
}

/// Only the documented native command set is forwarded; dialogs stay in the editor.
pub(crate) fn forward(invoke: tauri::ipc::Invoke) -> Option<tauri::ipc::Invoke> {
    if !is_service_command(invoke.message.command()) {
        return Some(invoke);
    }
    let app = invoke.message.webview().app_handle().clone();
    if *app.state::<crate::app_mode::AppMode>() != crate::app_mode::AppMode::Graph {
        return Some(invoke);
    }
    let name = invoke.message.command().to_owned();
    let tauri::ipc::InvokeBody::Json(arguments) = invoke.message.payload() else {
        invoke
            .resolver
            .reject("background commands require JSON arguments");
        return None;
    };
    let arguments = arguments.clone();
    invoke.resolver.respond_async(async move {
        let connection = app
            .try_state::<Connection>()
            .map(|state| state.inner().clone())
            .ok_or_else(|| {
                tauri::ipc::InvokeError(Value::String("background controller unavailable".into()))
            })?;
        match connection.exchange(&name, arguments).await {
            Ok(response) => response,
            Err(reason) => forward_transport_error(&name, &reason),
        }
        .map_err(tauri::ipc::InvokeError)
    });
    None
}

/// A fresh controller activation restores installed devices once. Reopening the
/// editor never reapplies Driver state or changes an existing graph.
pub(crate) fn restore_driver(app: &tauri::AppHandle) {
    if crate::driver_control::installed_driver_version().is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        #[cfg(target_os = "macos")]
        let result = app
            .state::<crate::engine_lifecycle::EngineLifecycle>()
            .restore_driver(mixrelay_driver_adapters::DriverClient::session)
            .await;
        #[cfg(target_os = "linux")]
        let result = match mixrelay_driver_adapters::DriverClient::session() {
            Ok(client) => client
                .set_active(true)
                .await
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        match result {
            Ok(Some(snapshot)) => {
                if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
                    menu.observe_snapshot(&snapshot);
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "Driver activation unavailable; physical audio remains available");
            }
        }
        drop(app);
    });
}

/// Reconnecting is an explicit editor action; background polling never restarts stopped audio.
pub(crate) fn reopen_editor(app: &tauri::AppHandle) {
    if let Err(error) = crate::app_menu::show_main_window(app) {
        tracing::error!(%error, "could not reopen the editor");
    }
    // The visible startup gate owns explicit reconnect attempts.
}
