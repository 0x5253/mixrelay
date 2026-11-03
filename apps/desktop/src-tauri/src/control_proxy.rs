use reqwest::{Client, Method};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

#[cfg(target_os = "macos")]
use crate::macos_audio::ApplicationCaptureState;
#[cfg(target_os = "macos")]
mod application_capture;
mod exchange;
pub(crate) mod local;
pub(crate) mod node_control;
pub(crate) mod recorder;
pub(crate) mod recording_settings;
pub(crate) mod telemetry;
pub(crate) mod workspace;

/// Requests reach the engine over its user-owned Unix socket; this authority only names it.
const CONTROL_ORIGIN: &str = mixrelay_local_socket::ENGINE_AUTHORITY;
/// The engine's server-sent meter windows (`mixrelay_control_server::METER_TELEMETRY_ROUTE`).
const METER_TELEMETRY_ROUTE: &str = "/api/v1/telemetry";
const PROCESSOR_RETRY_ROUTE: &str = "/api/v1/processors/retry";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProcessorRetryRequest {
    expected_catalog_revision: u64,
    expected_instance_id: String,
    expected_revision: u64,
    graph_id: String,
    node_id: String,
}

fn processor_retry_body(request: &ProcessorRetryRequest) -> Result<Value, String> {
    serde_json::to_value(request)
        .map_err(|error| format!("processor retry request could not be encoded: {error}"))
}
const GRAPH_CATALOG_ROUTE: &str = "/api/v1/graphs";

fn graph_layout_route(graph_id: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(CONTROL_ORIGIN).map_err(|error| error.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "invalid control origin".to_owned())?
        .extend(["api", "v1", "graphs", graph_id, "layout"]);
    Ok(url.path().to_owned())
}

// Layout writes do not prepare macOS application taps or submit audio revisions.
#[tauri::command]
pub(crate) async fn control_get_graph_layout(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, &graph_layout_route(&graph_id)?, None)
        .await
}

#[tauri::command]
pub(crate) async fn control_set_graph_layout(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::PUT, &graph_layout_route(&graph_id)?, Some(request))
        .await
}

#[derive(Debug, Clone)]
pub(crate) struct ControlProxyClient {
    client: Client,
    engine: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    pub(crate) background: std::sync::Arc<std::sync::Mutex<Option<crate::background::Connection>>>,
    #[cfg(target_os = "macos")]
    capture_gate: Arc<tauri::async_runtime::Mutex<()>>,
    closing: std::sync::Arc<std::sync::atomic::AtomicBool>,
    unavailable: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The client reaches the engine only through its socket. Without one (no session
    /// directory, or no client), every request is rejected here and never sent elsewhere.
    connected: bool,
    requests: std::sync::Arc<tokio::sync::RwLock<()>>,
    operation_liveness: std::sync::Arc<tokio::sync::Notify>,
    recordings: std::sync::Arc<std::sync::RwLock<recording_settings::RecordingStore>>,
    recording_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    take_identities: Arc<recording_settings::NativeTakeIdentities>,
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
}

impl ControlProxyClient {
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
            engine: std::sync::Arc::default(),
            connected: client.is_some(),
            client: client.unwrap_or_default(),
            background: std::sync::Arc::default(),
            closing: std::sync::Arc::default(),
            unavailable: std::sync::Arc::default(),
            requests: std::sync::Arc::default(),
            operation_liveness: std::sync::Arc::default(),
            recordings: std::sync::Arc::default(),
            recording_gate: std::sync::Arc::default(),
            take_identities: Arc::new(recording_settings::NativeTakeIdentities::new()),
            #[cfg(target_os = "macos")]
            capture_gate: Arc::default(),
        }
    }
}

#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlProxyResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) engine: Option<String>,
    pub(crate) status: u16,
    pub(crate) body: Value,
    #[cfg(target_os = "macos")]
    #[serde(skip)]
    delivery: RequestDelivery,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RequestDelivery {
    #[default]
    ServerResponse,
    NotSent,
    Uncertain,
}

impl ControlProxyClient {
    #[cfg(any(test, target_os = "linux"))]
    pub(crate) fn suspend_admission(&self) {
        self.unavailable
            .store(true, std::sync::atomic::Ordering::Release);
    }

    // Only the lifecycle owner calls this after draining old exchanges and obtaining
    // the replacement child's private readiness acknowledgement. Quit is irreversible.
    #[cfg(any(test, target_os = "linux"))]
    pub(crate) async fn resume_admission(&self) -> bool {
        let _requests = self.requests.write().await;
        if self.closing.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        self.unavailable
            .store(false, std::sync::atomic::Ordering::Release);
        true
    }

    pub(crate) fn close_admission(&self) {
        self.closing
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn drain(&self) -> Result<(), String> {
        wait_for_controls(
            async {
                // Same order as capture-aware commands: preparation, then HTTP admission.
                #[cfg(target_os = "macos")]
                let _capture = self.capture_gate.lock().await;
                let _recordings = self.recording_gate.lock().await;
                let _requests = self.requests.write().await;
            },
            &self.operation_liveness,
        )
        .await
    }

    pub(crate) async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<ControlProxyResponse, String> {
        let background = self
            .background
            .lock()
            .map_err(|_| "background connection lock poisoned")?
            .clone();
        if let Some(connection) = background {
            let value = connection
                .invoke(
                    "engine_request",
                    serde_json::json!({"method":method.as_str(),"path":path,"body":body}),
                )
                .await
                .map_err(|error| error.to_string())?;
            return serde_json::from_value(value).map_err(|error| error.to_string());
        }
        let _admission = self.requests.read().await;
        if !self.connected
            || self.closing.load(std::sync::atomic::Ordering::Acquire)
            || self.unavailable.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(rejected("control_unavailable"));
        }
        exchange::request(self, CONTROL_ORIGIN, method, path, body).await
    }

    /// Opens the engine's pushed meter windows. The stream is long-lived, so it holds
    /// no request admission: shutdown never waits for a subscriber.
    pub(crate) async fn meter_stream(&self) -> Result<reqwest::Response, String> {
        if !self.connected
            || self.closing.load(std::sync::atomic::Ordering::Acquire)
            || self.unavailable.load(std::sync::atomic::Ordering::Acquire)
        {
            return Err("control unavailable".to_owned());
        }
        let response = self
            .client
            .get(format!("{CONTROL_ORIGIN}{METER_TELEMETRY_ROUTE}"))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        exchange::observe_engine(self, &response);
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(format!("meter telemetry HTTP {}", response.status()))
        }
    }
}

// A receipt proves that the server still owns the accepted operation, not that
// native work has completed. Keep shutdown ownership while that observation is
// live; individual exchanges and native providers retain their own deadlines.
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
            () = tokio::time::sleep(std::time::Duration::from_secs(35)) => {
                return Err("Controls are still finishing; audio ownership was retained.".to_owned());
            }
        }
    }
}

fn rejected(code: &str) -> ControlProxyResponse {
    ControlProxyResponse {
        engine: None,
        status: 409,
        body: serde_json::json!({"code": code, "parameters": {"nodeControlOutcome": "not_applied"}}),
        #[cfg(target_os = "macos")]
        delivery: RequestDelivery::NotSent,
    }
}

#[tauri::command]
pub(crate) async fn control_get_status(
    client: tauri::State<'_, ControlProxyClient>,
    app: tauri::AppHandle,
) -> Result<ControlProxyResponse, String> {
    use tauri::Manager as _;
    let response = client.request(Method::GET, "/api/v1/status", None).await?;
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
        .request(Method::PUT, "/api/v1/audio/session", Some(request))
        .await
}

#[tauri::command]
pub(crate) async fn control_set_engine_power(
    client: tauri::State<'_, ControlProxyClient>,
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<ControlProxyResponse, String> {
    #[cfg(target_os = "macos")]
    use tauri::Manager as _;
    #[cfg(target_os = "macos")]
    let captures = app.state::<Arc<ApplicationCaptureState>>();
    #[cfg(target_os = "linux")]
    let _ = app;
    #[cfg(target_os = "macos")]
    let _guard = client.capture_gate.lock().await;
    #[cfg(target_os = "macos")]
    if enabled {
        return application_capture::enable(&client, captures.inner()).await;
    }
    let response = client
        .request(
            Method::PUT,
            "/api/v1/audio/power",
            Some(serde_json::json!({"enabled":enabled})),
        )
        .await?;
    #[cfg(target_os = "macos")]
    return application_capture::synchronize_response(&client, captures.inner(), response).await;
    #[cfg(target_os = "linux")]
    Ok(response)
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_get_graph(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client.request(Method::GET, "/api/v1/graph", None).await
}

#[tauri::command]
pub(crate) async fn control_set_virtual_endpoint_sample_rate(
    client: tauri::State<'_, ControlProxyClient>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(
            Method::PUT,
            "/api/v1/audio/virtual-endpoint/format",
            Some(request),
        )
        .await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn control_get_graph(
    client: tauri::State<'_, ControlProxyClient>,
    captures: tauri::State<'_, Arc<ApplicationCaptureState>>,
) -> Result<ControlProxyResponse, String> {
    let _guard = client.capture_gate.lock().await;
    let response = client.request(Method::GET, "/api/v1/graph", None).await?;
    application_capture::synchronize_response(&client, captures.inner(), response).await
}

#[tauri::command]
pub(crate) async fn control_get_operation(
    client: tauri::State<'_, ControlProxyClient>,
    operation_id: String,
) -> Result<ControlProxyResponse, String> {
    let mut url = reqwest::Url::parse(CONTROL_ORIGIN).map_err(|error| error.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "invalid control origin")?
        .extend(["api", "v1", "operations", &operation_id]);
    client.request(Method::GET, url.path(), None).await
}

#[tauri::command]
pub(crate) async fn control_get_graph_status(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, "/api/v1/graph/status", None)
        .await
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

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_get_graph_catalog(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client.request(Method::GET, GRAPH_CATALOG_ROUTE, None).await
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn control_get_graph_catalog(
    client: tauri::State<'_, ControlProxyClient>,
    captures: tauri::State<'_, Arc<ApplicationCaptureState>>,
) -> Result<ControlProxyResponse, String> {
    let _guard = client.capture_gate.lock().await;
    let response = client
        .request(Method::GET, GRAPH_CATALOG_ROUTE, None)
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
        .request(Method::PUT, GRAPH_CATALOG_ROUTE, Some(request))
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
        .request(Method::PUT, GRAPH_CATALOG_ROUTE, Some(request))
        .await?;
    application_capture::synchronize_response(&client, captures.inner(), response).await
}

#[tauri::command]
pub(crate) async fn control_get_meters(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client.request(Method::GET, "/api/v1/meters", None).await
}

#[tauri::command]
pub(crate) async fn control_get_processors(
    client: tauri::State<'_, ControlProxyClient>,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, "/api/v1/processors", None)
        .await
}

#[tauri::command]
pub(crate) async fn control_retry_processor(
    client: tauri::State<'_, ControlProxyClient>,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    let parsed: ProcessorRetryRequest = serde_json::from_value(request.clone())
        .map_err(|error| format!("invalid processor retry request: {error}"))?;
    let mut body = processor_retry_body(&parsed)?;
    if let Some(id) = request.get("clientOperationId") {
        body["clientOperationId"] = id.clone();
    }
    client
        .request(Method::PUT, PROCESSOR_RETRY_ROUTE, Some(body))
        .await
}

#[tauri::command]
pub(crate) async fn control_get_processor_editor(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
) -> Result<ControlProxyResponse, String> {
    let node_id = percent_encode_path_segment(&node_id);
    client
        .request(
            Method::GET,
            &format!("/api/v1/processors/{node_id}/editor"),
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_rescan_processors(
    client: tauri::State<'_, ControlProxyClient>,
    paths: Vec<String>,
) -> Result<ControlProxyResponse, String> {
    client
        .request(
            Method::PUT,
            "/api/v1/processors/scan",
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
    let node_id = percent_encode_path_segment(&node_id);
    let path = format!("/api/v1/processors/{node_id}/parameters/{parameter_id}");
    client
        .request(
            Method::PUT,
            &path,
            Some(serde_json::json!({ "value": value, "expectedRevision": expected_revision })),
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_set_processor_editor(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
    open: bool,
    expected_revision: u64,
) -> Result<ControlProxyResponse, String> {
    let node_id = percent_encode_path_segment(&node_id);
    client
        .request(
            Method::PUT,
            &format!("/api/v1/processors/{node_id}/editor"),
            Some(serde_json::json!({ "open": open, "expectedRevision": expected_revision })),
        )
        .await
}

fn percent_encode_path_segment(value: &str) -> String {
    value.bytes().fold(String::new(), |mut encoded, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
        encoded
    })
}

#[tauri::command]
pub(crate) async fn control_get_processor_editor_frame(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
) -> Result<ControlProxyResponse, String> {
    let node_id = percent_encode_path_segment(&node_id);
    client
        .request(
            Method::GET,
            &format!("/api/v1/processors/{node_id}/editor/frame"),
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_send_processor_editor_input(
    client: tauri::State<'_, ControlProxyClient>,
    node_id: String,
    input: serde_json::Value,
) -> Result<ControlProxyResponse, String> {
    let node_id = percent_encode_path_segment(&node_id);
    client
        .request(
            Method::POST,
            &format!("/api/v1/processors/{node_id}/editor/input"),
            Some(input),
        )
        .await
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn control_apply_graph(
    client: tauri::State<'_, ControlProxyClient>,
    document: Value,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::PUT, "/api/v1/graph", Some(document))
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

impl ControlProxyResponse {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[cfg(target_os = "macos")]
fn application_capture_error(reason: &str) -> ControlProxyResponse {
    ControlProxyResponse {
        engine: None,
        status: 503,
        delivery: RequestDelivery::NotSent,
        body: serde_json::json!({
            "code": "audio_source_unavailable",
            "parameters": { "reason": reason },
        }),
    }
}

#[cfg(test)]
mod layout_tests {
    #[tokio::test]
    async fn without_an_engine_socket_nothing_is_sent() {
        let client = super::ControlProxyClient::with_socket(None);
        let rejected = client
            .request(reqwest::Method::GET, "/api/v1/status", None)
            .await
            .expect("rejected locally");
        assert_eq!(rejected.status, 409);
        assert!(client.meter_stream().await.is_err());
    }

    #[tokio::test]
    async fn recovery_does_not_reopen_shutdown_admission() {
        let client = super::ControlProxyClient::with_socket(None);
        client.suspend_admission();
        let rejected = client
            .request(reqwest::Method::GET, "/api/v1/status", None)
            .await
            .expect("unavailable requests are rejected locally");
        assert_eq!(rejected.status, 409);
        assert!(client.resume_admission().await);
        assert!(
            !client
                .unavailable
                .load(std::sync::atomic::Ordering::Acquire)
        );
        client.suspend_admission();
        client.close_admission();
        assert!(!client.resume_admission().await);
        let rejected = client
            .request(reqwest::Method::GET, "/api/v1/status", None)
            .await
            .expect("shutdown remains closed");
        assert_eq!(rejected.status, 409);
    }

    #[tokio::test]
    async fn recovery_waits_for_admitted_requests_and_shutdown_wins() {
        let client = super::ControlProxyClient::with_socket(None);
        let admitted = client.requests.read().await;
        client.suspend_admission();
        let recovery = tokio::spawn({
            let client = client.clone();
            async move { client.resume_admission().await }
        });
        tokio::task::yield_now().await;
        assert!(!recovery.is_finished());
        client.close_admission();
        drop(admitted);
        assert!(!recovery.await.expect("recovery task"));
    }

    #[test]
    fn processor_retry_proxy_preserves_opaque_identity_and_revision_contract() {
        let request = super::ProcessorRetryRequest {
            expected_catalog_revision: 6,
            expected_instance_id: "18446744073709551615".to_owned(),
            expected_revision: 3,
            graph_id: "graph-a".to_owned(),
            node_id: "provider-a".to_owned(),
        };
        let body = super::processor_retry_body(&request).expect("request serializes");

        assert_eq!(super::PROCESSOR_RETRY_ROUTE, "/api/v1/processors/retry");
        assert_eq!(
            body,
            serde_json::json!({
                "expectedCatalogRevision": 6,
                "expectedInstanceId": "18446744073709551615",
                "expectedRevision": 3,
                "graphId": "graph-a",
                "nodeId": "provider-a",
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn live_receipts_extend_drain_without_releasing_accepted_ownership() {
        let client = super::ControlProxyClient::with_socket(None);
        let accepted = client.requests.read().await;
        client.close_admission();
        let draining = tokio::spawn({
            let client = client.clone();
            async move { client.drain().await }
        });
        tokio::task::yield_now().await;
        for _ in 0..3 {
            tokio::time::advance(std::time::Duration::from_secs(30)).await;
            assert!(!draining.is_finished());
            client.operation_liveness.notify_waiters();
            tokio::task::yield_now().await;
        }
        drop(accepted);
        draining
            .await
            .expect("drain task")
            .expect("live request drained");
    }

    #[tokio::test(start_paused = true)]
    async fn missing_liveness_reports_retained_ownership_without_cancelling_request() {
        let client = super::ControlProxyClient::with_socket(None);
        let accepted = client.requests.read().await;
        let draining = tokio::spawn({
            let client = client.clone();
            async move { client.drain().await }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(36)).await;
        assert!(draining.await.expect("drain task").is_err());
        assert!(client.requests.try_write().is_err());
        drop(accepted);
        client.drain().await.expect("later completion can drain");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn closing_rejects_requests_and_waits_for_accepted_work() {
        let client = super::ControlProxyClient::with_socket(None);
        let accepted = client.requests.read().await;
        client.close_admission();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), client.drain())
                .await
                .is_err()
        );
        drop(accepted);
        client.drain().await.expect("drained");
        let response = client
            .request(super::Method::PUT, "/not-sent", None)
            .await
            .expect("response");
        assert_eq!(response.delivery, super::RequestDelivery::NotSent);
        assert_eq!(response.status, 409);
    }

    #[test]
    fn layout_route_encodes_one_graph_segment_without_changing_the_endpoint() {
        assert_eq!(
            super::graph_layout_route("graph-1").unwrap(),
            "/api/v1/graphs/graph-1/layout"
        );
        assert_eq!(
            super::graph_layout_route("a/b?c#d").unwrap(),
            "/api/v1/graphs/a%2Fb%3Fc%23d/layout"
        );
    }
}
