use super::{ControlProxyClient, ControlProxyResponse, Method, Value};

pub(super) fn route(graph_id: &str, node_id: Option<&str>) -> Result<String, String> {
    let mut url = reqwest::Url::parse(super::CONTROL_ORIGIN).map_err(|error| error.to_string())?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|()| "invalid control origin")?;
    segments.extend(["api", "v1", "graphs", graph_id, "nodes"]);
    if let Some(node) = node_id {
        segments.extend([node, "control"]);
    } else {
        segments.push("controls");
    }
    drop(segments);
    Ok(url.path().to_owned())
}

/// Reads supported controls and authoritative states for one graph.
#[tauri::command]
pub(crate) async fn control_get_node_controls(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
) -> Result<ControlProxyResponse, String> {
    client
        .request(Method::GET, &route(&graph_id, None)?, None)
        .await
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
        client
            .request(
                Method::PUT,
                &route(&graph_id, Some(&node_id))?,
                Some(request),
            )
            .await
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
        if control.enabled.is_none() {
            // Gain/mute cannot change tap ownership; avoid the capture transaction gate.
            return self
                .request(Method::PUT, &route(graph_id, Some(node_id))?, Some(request))
                .await;
        }
        let Ok(_guard) = self.capture_gate.try_lock() else {
            return Ok(super::rejected("graph_exchange_busy"));
        };
        super::application_capture::node_control(
            self,
            captures,
            route(graph_id, Some(node_id))?,
            request,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::route;

    #[test]
    fn catalog_and_command_routes_keep_graph_and_node_identity_separate() {
        assert_eq!(
            route("main", None).unwrap(),
            "/api/v1/graphs/main/nodes/controls"
        );
        assert_eq!(
            route("graph/2", Some("processor/?#")).unwrap(),
            "/api/v1/graphs/graph%2F2/nodes/processor%2F%3F%23/control"
        );
    }
}
