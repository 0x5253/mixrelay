use mixrelay_desktop_core::engine_control::routes;

use super::{ControlProxyClient, ControlProxyResponse, Method, Value};

/// Reads supported controls and authoritative states for one graph.
#[tauri::command]
pub(crate) async fn control_get_node_controls(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
) -> Result<ControlProxyResponse, String> {
    let route = routes::node_controls(&graph_id).map_err(|error| error.to_string())?;
    client.request(Method::GET, &route, None).await
}

/// Applies node control through the shared capture reservation/publication path.
/// Returns transport/preparation errors without committing partial native captures.
#[tauri::command]
pub(crate) async fn control_update_node_control(
    client: tauri::State<'_, ControlProxyClient>,
    app: tauri::AppHandle,
    graph_id: String,
    node_id: String,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager as _;
        let captures = app.state::<std::sync::Arc<crate::macos_audio::ApplicationCaptureState>>();
        client
            .update_node_control(captures.inner(), &graph_id, &node_id, request)
            .await
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let route = routes::node_control(&graph_id, &node_id).map_err(|error| error.to_string())?;
        client.request(Method::PUT, &route, Some(request)).await
    }
}

#[cfg(target_os = "macos")]
impl ControlProxyClient {
    /// Shared UI/local-controller entry point. Busy commands are rejected, never queued
    /// for later execution against a graph the caller is no longer observing.
    pub(super) async fn update_node_control(
        &self,
        captures: &std::sync::Arc<crate::macos_audio::ApplicationCaptureState>,
        graph_id: &str,
        node_id: &str,
        request: Value,
    ) -> Result<ControlProxyResponse, String> {
        let control: mixrelay_control_protocol::NodeControlRequest =
            serde_json::from_value(request.clone())
                .map_err(|error| format!("invalid node control request: {error}"))?;
        let route = routes::node_control(graph_id, node_id).map_err(|error| error.to_string())?;
        if control.enabled.is_none() {
            // Gain/mute cannot change tap ownership; avoid the capture transaction gate.
            return self.request(Method::PUT, &route, Some(request)).await;
        }
        let Ok(_guard) = self.capture_gate.try_lock() else {
            return Ok(ControlProxyResponse::rejected("graph_exchange_busy"));
        };
        super::application_capture::node_control(self, captures, route, request).await
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_enable_command_during_another_capture_transaction_is_refused_not_queued() {
        let client = ControlProxyClient::with_socket(None);
        let captures = std::sync::Arc::default();
        let _transaction = client.capture_gate.lock().await;
        let busy = client
            .update_node_control(
                &captures,
                "g",
                "n",
                serde_json::json!({"expectedRevision":1,"enabled":true}),
            )
            .await
            .unwrap();
        assert_eq!(busy.status, 409);
        assert_eq!(busy.body["code"], "graph_exchange_busy");
        assert_eq!(busy.body["parameters"]["nodeControlOutcome"], "not_applied");
    }

    #[tokio::test]
    async fn gain_and_mute_do_not_wait_for_the_capture_transaction() {
        let client = ControlProxyClient::with_socket(None);
        let captures = std::sync::Arc::default();
        let _transaction = client.capture_gate.lock().await;
        let sent = client
            .update_node_control(
                &captures,
                "g",
                "n",
                serde_json::json!({"expectedRevision":1,"muted":true}),
            )
            .await
            .unwrap();
        // Admitted past the gate; refused only because no engine socket exists.
        assert_eq!(sent.body["code"], "control_unavailable");
    }

    #[tokio::test]
    async fn a_malformed_node_control_is_refused_before_any_request() {
        let client = ControlProxyClient::with_socket(None);
        let error = client
            .update_node_control(
                &std::sync::Arc::default(),
                "g",
                "n",
                serde_json::json!({"enabled":"yes"}),
            )
            .await
            .unwrap_err();
        assert!(error.starts_with("invalid node control request"));
    }
}
