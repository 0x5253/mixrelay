//! The editor's control client of the engine and the Tauri commands that use it. Routes,
//! receipts, failure reports and capture settlement are protocol policy in
//! `mixrelay_desktop_core::engine_control`; this module and its children own admission,
//! the HTTP exchange and native capture.
use std::sync::{Arc, Mutex};

use mixrelay_desktop_core::engine_control::routes;
use reqwest::{Client, Method};
use serde_json::Value;

#[cfg(target_os = "macos")]
use crate::macos_audio::ApplicationCaptureState;
use admission::Admission;
use response::ControlProxyResponse;

mod admission;
#[cfg(target_os = "macos")]
mod application_capture;
mod exchange;
pub(crate) mod local;
pub(crate) mod node_control;
pub(crate) mod recorder;
pub(crate) mod recording_settings;
mod response;
pub(crate) mod telemetry;
pub(crate) mod workspace;

/// Requests reach the engine over its user-owned Unix socket; this authority only names it.
const CONTROL_ORIGIN: &str = mixrelay_local_socket::ENGINE_AUTHORITY;
/// How long shutdown waits for controls without a liveness pulse before giving up.
const CONTROL_LIVENESS_LIMIT: std::time::Duration = std::time::Duration::from_secs(35);

#[derive(Debug, Clone)]
pub(crate) struct ControlProxyClient {
    client: Client,
    /// The engine incarnation that answered last; mutations are fenced to it.
    engine: Arc<Mutex<Option<String>>>,
    /// The background runtime that owns the engine; the editor forwards requests to it.
    pub(crate) background: Arc<Mutex<Option<crate::background::Connection>>>,
    admission: Arc<Admission>,
    /// Pulsed while an accepted operation is still observed, extending shutdown waits.
    operation_liveness: Arc<tokio::sync::Notify>,
    /// Serializes capture preparation with the submissions that publish it.
    #[cfg(target_os = "macos")]
    capture_gate: Arc<tauri::async_runtime::Mutex<()>>,
    recordings: Arc<recording_settings::Recordings>,
}

impl ControlProxyClient {
    pub(crate) fn for_session() -> Self {
        // The session's socket: installed services never see an override set for the app.
        let socket = mixrelay_local_socket::session_socket_directory()
            .map(|directory| directory.join(mixrelay_local_socket::ENGINE_SOCKET))
            .inspect_err(|error| tracing::error!(%error, "no engine control socket"))
            .ok();
        Self::with_socket(socket)
    }

    /// A client of the engine socket at `socket`; without one, every request is rejected.
    pub(crate) fn with_socket(socket: Option<std::path::PathBuf>) -> Self {
        let client = socket.and_then(|socket| {
            Client::builder()
                // Ambiguous mutations are resolved by ID, never sent again by HTTP retry.
                .retry(reqwest::retry::never())
                .unix_socket(socket)
                .no_proxy()
                .build()
                .inspect_err(|error| tracing::error!(%error, "no engine control client"))
                .ok()
        });
        Self {
            admission: Arc::new(Admission::new(client.is_some())),
            client: client.unwrap_or_default(),
            engine: Arc::default(),
            background: Arc::default(),
            operation_liveness: Arc::default(),
            #[cfg(target_os = "macos")]
            capture_gate: Arc::default(),
            recordings: Arc::default(),
        }
    }

    #[cfg(any(test, target_os = "linux"))]
    pub(crate) fn suspend_admission(&self) {
        self.admission.suspend();
    }

    /// Only the lifecycle owner calls this after draining old exchanges and obtaining the
    /// replacement child's private readiness acknowledgement. Quit is irreversible.
    #[cfg(any(test, target_os = "linux"))]
    pub(crate) async fn resume_admission(&self) -> bool {
        self.admission.resume().await
    }

    pub(crate) fn close_admission(&self) {
        self.admission.close();
    }

    /// Waits for capture preparation, recording commits and admitted exchanges, in the
    /// order capture-aware commands take them.
    pub(crate) async fn drain(&self) -> Result<(), String> {
        wait_for_controls(
            async {
                #[cfg(target_os = "macos")]
                let _capture = self.capture_gate.lock().await;
                let _recordings = self.recordings.lock_commits().await;
                self.admission.quiesce().await;
            },
            &self.operation_liveness,
        )
        .await
    }

    /// Sends one request: through the background runtime when the editor has one,
    /// otherwise to the engine socket while admission is open.
    pub(crate) async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<ControlProxyResponse, String> {
        if let Some(connection) = self.background_connection()? {
            let value = connection
                .invoke(
                    "engine_request",
                    serde_json::json!({"method":method.as_str(),"path":path,"body":body}),
                )
                .await
                .map_err(|error| error.to_string())?;
            return serde_json::from_value(value).map_err(|error| error.to_string());
        }
        let Some(_admitted) = self.admission.enter().await else {
            return Ok(ControlProxyResponse::rejected("control_unavailable"));
        };
        let timer = crate::async_time::TokioDelay::new(mixrelay_time::StdClock::new());
        exchange::Exchange::new(self, CONTROL_ORIGIN, &timer)
            .request(method, path, body)
            .await
    }

    fn background_connection(&self) -> Result<Option<crate::background::Connection>, String> {
        Ok(self
            .background
            .lock()
            .map_err(|_| "background connection lock poisoned")?
            .clone())
    }

    /// Opens the engine's pushed meter windows. The stream is long-lived, so it holds
    /// no request admission: shutdown never waits for a subscriber.
    pub(crate) async fn meter_stream(&self) -> Result<reqwest::Response, String> {
        if !self.admission.is_open() {
            return Err("control unavailable".to_owned());
        }
        let response = self
            .client
            .get(format!("{CONTROL_ORIGIN}{}", routes::TELEMETRY))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        exchange::observe_engine(self, response.headers());
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!("meter telemetry HTTP {}", response.status()))
        }
    }
}

/// Waits for `completion` while accepted operations keep pulsing `liveness`.
///
/// A receipt proves that the server still owns the accepted operation, not that native
/// work has completed. Keep shutdown ownership while that observation is live; individual
/// exchanges and native providers retain their own deadlines.
pub(crate) async fn wait_for_controls<T>(
    completion: impl std::future::Future<Output = T>,
    liveness: &tokio::sync::Notify,
) -> Result<T, String> {
    tokio::pin!(completion);
    loop {
        let pulse = liveness.notified();
        tokio::select! {
            biased;
            result = &mut completion => return Ok(result),
            () = pulse => {},
            () = tokio::time::sleep(CONTROL_LIVENESS_LIMIT) => {
                return Err("Controls are still finishing; audio ownership was retained.".to_owned());
            }
        }
    }
}

/// Releases capture owners bound to a lost engine before its successor publishes a
/// manifest; the successor's captures are then prepared like a cold start.
#[cfg(target_os = "macos")]
pub(crate) async fn release_lost_engine_captures(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager as _;
    let client = app.state::<ControlProxyClient>();
    let captures = Arc::clone(app.state::<Arc<ApplicationCaptureState>>().inner());
    let _guard = client.capture_gate.lock().await;
    tauri::async_runtime::spawn_blocking(move || captures.release_all())
        .await
        .map_err(|error| format!("capture release task failed: {error}"))?
}

// Tauri commands. Each is one engine request; macOS commands that can change which
// applications the engine captures also reconcile desktop-owned taps under the capture gate.

#[tauri::command]
pub(crate) async fn control_get_status(
    client: tauri::State<'_, ControlProxyClient>,
    app: tauri::AppHandle,
) -> Result<ControlProxyResponse, String> {
    use tauri::Manager as _;
    let response = client.request(Method::GET, routes::STATUS, None).await?;
    #[cfg(target_os = "macos")]
    let response = {
        let captures = app.state::<Arc<ApplicationCaptureState>>();
        application_capture::power_status(&client, captures.inner(), response).await?
    };
    if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
        menu.observe_audio_power(if response.is_success() {
            serde_json::from_value(response.body["enginePower"].clone()).ok()
        } else {
            None
        });
    }
    Ok(response)
}

#[tauri::command]
pub(crate) async fn control_reconfigure_session(
    client: tauri::State<'_, ControlProxyClient>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::PUT, routes::AUDIO_SESSION, Some(request))
        .await
}

#[tauri::command]
pub(crate) async fn control_set_engine_power(
    client: tauri::State<'_, ControlProxyClient>,
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<ControlProxyResponse, String> {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager as _;
        let captures = app.state::<Arc<ApplicationCaptureState>>();
        let _guard = client.capture_gate.lock().await;
        if enabled {
            return application_capture::enable(&client, captures.inner()).await;
        }
        let response = set_engine_power(&client, enabled).await?;
        application_capture::synchronize_response(&client, captures.inner(), response).await
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        set_engine_power(&client, enabled).await
    }
}

async fn set_engine_power(
    client: &ControlProxyClient,
    enabled: bool,
) -> Result<ControlProxyResponse, String> {
    client
        .request(
            Method::PUT,
            routes::AUDIO_POWER,
            Some(serde_json::json!({"enabled":enabled})),
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_set_virtual_endpoint_sample_rate(
    client: tauri::State<'_, ControlProxyClient>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::PUT, routes::VIRTUAL_ENDPOINT_FORMAT, Some(request))
        .await
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_get_graph(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client.request(Method::GET, routes::GRAPH, None).await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn control_get_graph(
    client: tauri::State<'_, ControlProxyClient>,
    captures: tauri::State<'_, Arc<ApplicationCaptureState>>,
) -> Result<ControlProxyResponse, String> {
    let _guard = client.capture_gate.lock().await;
    let response = client.request(Method::GET, routes::GRAPH, None).await?;
    application_capture::synchronize_response(&client, captures.inner(), response).await
}

#[tauri::command]
pub(crate) async fn control_get_graph_status(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, routes::GRAPH_STATUS, None)
        .await
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_get_graph_catalog(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, routes::GRAPH_CATALOG, None)
        .await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn control_get_graph_catalog(
    client: tauri::State<'_, ControlProxyClient>,
    captures: tauri::State<'_, Arc<ApplicationCaptureState>>,
) -> Result<ControlProxyResponse, String> {
    let _guard = client.capture_gate.lock().await;
    let response = client
        .request(Method::GET, routes::GRAPH_CATALOG, None)
        .await?;
    application_capture::synchronize_response(&client, captures.inner(), response).await
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_mutate_graph_catalog(
    client: tauri::State<'_, ControlProxyClient>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::PUT, routes::GRAPH_CATALOG, Some(request))
        .await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn control_mutate_graph_catalog(
    client: tauri::State<'_, ControlProxyClient>,
    captures: tauri::State<'_, Arc<ApplicationCaptureState>>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    let _guard = client.capture_gate.lock().await;
    let response = client
        .request(Method::PUT, routes::GRAPH_CATALOG, Some(request))
        .await?;
    application_capture::synchronize_response(&client, captures.inner(), response).await
}

// Layout writes do not prepare macOS application taps or submit audio revisions.
#[tauri::command]
pub(crate) async fn control_get_graph_layout(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
) -> Result<ControlProxyResponse, String> {
    let route = routes::graph_layout(&graph_id).map_err(|error| error.to_string())?;
    client.request(Method::GET, &route, None).await
}

#[tauri::command]
pub(crate) async fn control_set_graph_layout(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    let route = routes::graph_layout(&graph_id).map_err(|error| error.to_string())?;
    client.request(Method::PUT, &route, Some(request)).await
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_apply_graph(
    client: tauri::State<'_, ControlProxyClient>,
    document: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::PUT, routes::GRAPH, Some(document))
        .await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn control_apply_graph(
    client: tauri::State<'_, ControlProxyClient>,
    captures: tauri::State<'_, Arc<ApplicationCaptureState>>,
    document: Value,
) -> Result<ControlProxyResponse, String> {
    let _guard = client.capture_gate.lock().await;
    application_capture::apply(&client, captures.inner(), document).await
}

#[tauri::command]
pub(crate) async fn control_get_operation(
    client: tauri::State<'_, ControlProxyClient>,
    operation_id: String,
) -> Result<ControlProxyResponse, String> {
    let route = routes::operation(&operation_id).map_err(|error| error.to_string())?;
    client.request(Method::GET, &route, None).await
}

#[tauri::command]
pub(crate) async fn control_get_meters(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client.request(Method::GET, routes::METERS, None).await
}

#[tauri::command]
pub(crate) async fn control_get_processors(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client.request(Method::GET, routes::PROCESSORS, None).await
}

#[tauri::command]
pub(crate) async fn control_retry_processor(
    client: tauri::State<'_, ControlProxyClient>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(
            Method::PUT,
            routes::PROCESSOR_RETRY,
            Some(processor_retry_body(&request)?),
        )
        .await
}

/// The retry identity the engine checks, in the exact shape it accepts.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcessorRetryRequest {
    expected_catalog_revision: u64,
    expected_instance_id: String,
    expected_revision: u64,
    graph_id: String,
    node_id: String,
}

/// Validates a retry against the engine's contract, keeping the editor's operation ID.
fn processor_retry_body(request: &Value) -> Result<Value, String> {
    let mut retry = request.clone();
    let operation = retry
        .as_object_mut()
        .and_then(|fields| fields.remove("clientOperationId"));
    let parsed: ProcessorRetryRequest = serde_json::from_value(retry)
        .map_err(|error| format!("invalid processor retry request: {error}"))?;
    let mut body = serde_json::to_value(parsed)
        .map_err(|error| format!("processor retry request could not be encoded: {error}"))?;
    if let Some(id) = operation {
        body["clientOperationId"] = id;
    }
    Ok(body)
}

#[tauri::command]
pub(crate) async fn control_rescan_processors(
    client: tauri::State<'_, ControlProxyClient>,
    paths: Vec<String>,
) -> Result<ControlProxyResponse, String> {
    client
        .request(
            Method::PUT,
            routes::PROCESSOR_SCAN,
            Some(serde_json::json!({ "paths": paths })),
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_set_processor_parameter(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
    parameter_id: u32,
    value: f64,
    expected_revision: u64,
) -> Result<ControlProxyResponse, String> {
    let route =
        routes::processor_parameter(&node_id, parameter_id).map_err(|error| error.to_string())?;
    client
        .request(
            Method::PUT,
            &route,
            Some(serde_json::json!({ "value": value, "expectedRevision": expected_revision })),
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_get_processor_editor(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
) -> Result<ControlProxyResponse, String> {
    let route = routes::processor_editor(&node_id).map_err(|error| error.to_string())?;
    client.request(Method::GET, &route, None).await
}

#[tauri::command]
pub(crate) async fn control_set_processor_editor(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
    open: bool,
    expected_revision: u64,
) -> Result<ControlProxyResponse, String> {
    let route = routes::processor_editor(&node_id).map_err(|error| error.to_string())?;
    client
        .request(
            Method::PUT,
            &route,
            Some(serde_json::json!({ "open": open, "expectedRevision": expected_revision })),
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_get_processor_editor_frame(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
) -> Result<ControlProxyResponse, String> {
    let route = routes::processor_editor_frame(&node_id).map_err(|error| error.to_string())?;
    client.request(Method::GET, &route, None).await
}

#[tauri::command]
pub(crate) async fn control_send_processor_editor_input(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
    input: Value,
) -> Result<ControlProxyResponse, String> {
    let route = routes::processor_editor_input(&node_id).map_err(|error| error.to_string())?;
    client.request(Method::POST, &route, Some(input)).await
}

#[cfg(test)]
mod route_contract_tests;
#[cfg(test)]
mod tests;
