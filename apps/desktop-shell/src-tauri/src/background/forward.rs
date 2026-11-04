//! The editor sends native commands to the background controller; dialogs stay local.
use serde_json::{Value, json};
use tauri::Manager as _;

use super::{Connection, ConnectionFailure, command::NativeCommand};

/// Takes over a webview invocation the controller serves and returns any other one.
pub(crate) fn forward(invoke: tauri::ipc::Invoke) -> Option<tauri::ipc::Invoke> {
    if !NativeCommand::parse(invoke.message.command())
        .is_some_and(NativeCommand::is_forwarded_from_editor)
    {
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
            Err(reason) => transport_error(&name, &reason),
        }
        .map_err(tauri::ipc::InvokeError)
    });
    None
}

/// A catalog read lost in transport is answered as a retryable unavailable status.
pub(super) fn transport_error(name: &str, error: &ConnectionFailure) -> Result<Value, Value> {
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
