//! Local controllers use the same capture-aware command boundary as the desktop UI.
#[cfg(target_os = "macos")]
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mixrelay_control_protocol::{
    EngineControlSnapshot, NodeControlRequest, RecorderControlRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ControlProxyClient, ControlProxyResponse, Method, rejected};
#[cfg(target_os = "macos")]
use crate::macos_audio::ApplicationCaptureState;

mod meters;
pub(crate) mod midi;
mod outbound;
pub(crate) mod socket;

/// One request per connection. Callers refresh after any disconnect and never replay writes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub id: u64,
    command: Command,
}

/// How a connection handles a request.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RequestKind {
    /// Cancelled when the editor leaves or the runtime stops.
    Read,
    /// Always runs to completion once received; pending frames show it is alive.
    Mutation,
    /// Streams pushed telemetry under the request's ID until cancelled or ended.
    Subscribe,
    /// Ends the subscription with this ID. It has no reply.
    Cancel(u64),
}

impl Request {
    pub(super) fn kind(&self) -> RequestKind {
        match &self.command {
            Command::Snapshot {} => RequestKind::Read,
            Command::SubscribeTelemetry {} => RequestKind::Subscribe,
            Command::Cancel { subscription } => RequestKind::Cancel(*subscription),
            Command::Invoke { name, arguments } if cancellable_read(name, arguments) => {
                RequestKind::Read
            }
            _ => RequestKind::Mutation,
        }
    }
}

// Capture-aware reads may reconcile native owners and remain mutations. Linux
// discovery owns bounded child processes and must join them before shutdown.
// Only observation commands without that work can be cancelled with their connection.
fn cancellable_read(name: &str, arguments: &Value) -> bool {
    matches!(
        name,
        "control_get_graph_layout"
            | "control_get_graph_status"
            | "control_get_operation"
            | "control_get_meters"
            | "control_get_node_controls"
            | "control_get_recorder_status"
            | "recording_settings"
            | "workspace_path"
    ) || (cfg!(target_os = "macos") && name == "list_capturable_applications")
        || (name == "engine_request" && arguments["method"] == "GET")
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Snapshot {},
    SubscribeTelemetry {},
    Cancel {
        subscription: u64,
    },
    Invoke {
        name: String,
        arguments: Value,
    },
    Node {
        session: String,
        target: Target,
        request: NodeControlRequest,
    },
    Recorder {
        session: String,
        target: Target,
        request: RecorderControlRequest,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    graph_id: String,
    node_id: String,
}

pub(crate) struct LocalControl {
    client: ControlProxyClient,
    #[cfg(target_os = "macos")]
    captures: Arc<ApplicationCaptureState>,
    app: Option<tauri::AppHandle>,
    session: String,
}

impl LocalControl {
    pub(crate) fn new(
        client: ControlProxyClient,
        #[cfg(target_os = "macos")] captures: Arc<ApplicationCaptureState>,
    ) -> Result<Self, String> {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            #[cfg(target_os = "macos")]
            captures,
            app: None,
            session: format!("{}-{}", std::process::id(), started.as_nanos()),
        })
    }

    pub(crate) fn with_app(mut self, app: tauri::AppHandle) -> Self {
        self.app = Some(app);
        self
    }

    pub(super) async fn meter_stream(&self) -> Result<reqwest::Response, String> {
        self.client.meter_stream().await
    }

    /// Preserve native stop acknowledgement and menu observations when reads are pushed.
    #[cfg_attr(
        not(target_os = "macos"),
        expect(
            clippy::unused_async,
            reason = "the shared observation interface awaits native power acknowledgement on macOS"
        )
    )]
    pub(super) async fn observe_telemetry(
        &self,
        window: &mut mixrelay_control_protocol::ControlTelemetry,
    ) -> Result<(), String> {
        let Some(state) = &mut window.state else {
            return Ok(());
        };
        #[cfg(target_os = "macos")]
        {
            let response = ControlProxyResponse {
                engine: Some(window.engine_incarnation.clone()),
                status: 200,
                body: serde_json::to_value(&state.status).map_err(|error| error.to_string())?,
                delivery: super::RequestDelivery::ServerResponse,
            };
            let response =
                super::application_capture::power_status(&self.client, &self.captures, response)
                    .await?;
            if !response.is_success() {
                return Err("native power observation unavailable".to_owned());
            }
            state.status =
                serde_json::from_value(response.body).map_err(|error| error.to_string())?;
        }
        if let Some(app) = &self.app {
            use tauri::Manager as _;
            if let Some(menu) = app.try_state::<crate::status_menu::StatusMenu>() {
                menu.observe_audio_power(state.status.engine_power);
            }
        }
        Ok(())
    }

    pub(crate) async fn dispatch(&self, request: Request) -> Value {
        let response = self
            .execute(request.command)
            .await
            .unwrap_or_else(|reason| ControlProxyResponse {
                engine: None,
                status: 503,
                body: json!({"code":"control_unavailable", "parameters":{"reason":reason}}),
                #[cfg(target_os = "macos")]
                delivery: super::RequestDelivery::Uncertain,
            });
        // A response that cannot become JSON fails only its own request.
        let mut value = serde_json::to_value(response).unwrap_or_else(|error| {
            json!({
                "status": 500,
                "body": {
                    "code": "control_request_failed",
                    "parameters": {"reason": error.to_string()},
                },
            })
        });
        value["id"] = json!(request.id);
        value
    }

    async fn execute(&self, command: Command) -> Result<ControlProxyResponse, String> {
        match command {
            Command::Invoke { name, arguments } => {
                let app = self
                    .app
                    .as_ref()
                    .ok_or("background controller unavailable")?;
                let result = crate::background::dispatch(app, &name, arguments).await;
                Ok(ControlProxyResponse {
                    engine: None,
                    status: 200,
                    body: match result {
                        Ok(value) => json!({"ok":value}),
                        Err(error) => json!({"error":error}),
                    },
                    #[cfg(target_os = "macos")]
                    delivery: super::RequestDelivery::ServerResponse,
                })
            }
            Command::Snapshot {} => self.snapshot().await,
            // The socket streams subscriptions on their own connection before dispatch.
            Command::SubscribeTelemetry {} | Command::Cancel { .. } => {
                Err("subscriptions are handled by the connection".to_owned())
            }
            Command::Node {
                session,
                target,
                request,
            } => {
                if session != self.session {
                    return Ok(rejected("graph_revision_stale"));
                }
                #[cfg(target_os = "linux")]
                return self
                    .client
                    .request(
                        Method::PUT,
                        &super::node_control::route(&target.graph_id, Some(&target.node_id))?,
                        Some(serde_json::to_value(request).map_err(|error| error.to_string())?),
                    )
                    .await;
                #[cfg(target_os = "macos")]
                self.client
                    .update_node_control(
                        &self.captures,
                        &target.graph_id,
                        &target.node_id,
                        serde_json::to_value(request).map_err(|error| error.to_string())?,
                    )
                    .await
            }
            Command::Recorder {
                session,
                target,
                request,
            } => {
                if session != self.session || request.expected_recording.is_none() {
                    return Ok(rejected("graph_revision_stale"));
                }
                self.client
                    .submit_recorder(
                        &target.graph_id,
                        &target.node_id,
                        serde_json::to_value(request).map_err(|error| error.to_string())?,
                    )
                    .await
            }
        }
    }

    async fn snapshot(&self) -> Result<ControlProxyResponse, String> {
        let response = self
            .client
            .request(Method::GET, "/api/v1/controls/snapshot", None)
            .await?;
        let mut response = stamp_snapshot(response, &self.session)?;
        if response.is_success()
            && let Some(graphs) = response.body["graphs"].as_array_mut()
        {
            for graph in graphs {
                let graph_id = graph["id"].as_str().unwrap_or_default().to_owned();
                if let Some(nodes) = graph["nodes"].as_array_mut() {
                    for node in nodes.iter_mut().filter(|node| !node["recorder"].is_null()) {
                        let node_id = node["nodeId"].as_str().unwrap_or_default();
                        node["recordingSettings"] = serde_json::to_value(
                            self.client.recording_preferences(&graph_id, node_id)?,
                        )
                        .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        Ok(response)
    }
}

/// The engine owns consistency; the desktop adds only its connection-lifetime fence.
fn stamp_snapshot(
    mut response: ControlProxyResponse,
    session: &str,
) -> Result<ControlProxyResponse, String> {
    if !response.is_success() {
        return Ok(response);
    }
    let snapshot: EngineControlSnapshot = serde_json::from_value(response.body)
        .map_err(|error| format!("invalid control snapshot: {error}"))?;
    response.body = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    response.body["protocolVersion"] = json!(1);
    response.body["session"] = json!(session);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_adds_only_desktop_session_and_keeps_pending_execution_visible() {
        let mut body = json!({"revision":8,"activeRevision":7,"structureGeneration":3,
            "enginePower":{"enabled":true,"transitioning":false},"graphs":[]});
        let result = stamp_snapshot(
            ControlProxyResponse {
                engine: None,
                status: 200,
                body: body.clone(),
                #[cfg(target_os = "macos")]
                delivery: super::super::RequestDelivery::ServerResponse,
            },
            "session",
        )
        .expect("snapshot");
        body["session"] = json!("session");
        body["protocolVersion"] = json!(1);
        assert_eq!(result.status, 200);
        assert_eq!(result.body, body);
    }

    #[test]
    fn snapshot_failure_preserves_the_engine_error_without_turning_it_into_disconnect() {
        let body = json!({"code":"control_unavailable", "parameters":{"reason":"device fault"}});
        let result = stamp_snapshot(
            ControlProxyResponse {
                engine: None,
                status: 503,
                body: body.clone(),
                #[cfg(target_os = "macos")]
                delivery: super::super::RequestDelivery::ServerResponse,
            },
            "session",
        )
        .expect("engine response");
        assert_eq!(result.status, 503);
        assert_eq!(result.body, body);
    }

    #[test]
    fn only_explicit_bounded_commands_deserialize() {
        for value in [
            json!({"id":1,"command":{"type":"snapshot","url":"/private"}}),
            json!({"id":1,"command":{"type":"toggle"}}),
            json!({"id":1,"command":{"type":"node","session":"s","target":{"graphId":"g","nodeId":"n"},"request":{"enabled":true}}}),
        ] {
            assert!(serde_json::from_value::<Request>(value).is_err());
        }
        for (command, kind) in [
            (json!({"type":"snapshot"}), RequestKind::Read),
            (
                json!({"type":"subscribe_telemetry"}),
                RequestKind::Subscribe,
            ),
            (
                json!({"type":"cancel","subscription":7}),
                RequestKind::Cancel(7),
            ),
            (
                json!({"type":"invoke","name":"hello","arguments":null}),
                RequestKind::Mutation,
            ),
            (
                json!({"type":"invoke","name":"control_get_graph_status","arguments":null}),
                RequestKind::Read,
            ),
            (
                json!({"type":"invoke","name":"list_capturable_applications","arguments":null}),
                if cfg!(target_os = "linux") {
                    RequestKind::Mutation
                } else {
                    RequestKind::Read
                },
            ),
            (
                json!({"type":"invoke","name":"control_get_graph_catalog","arguments":null}),
                RequestKind::Mutation,
            ),
            (
                json!({"type":"invoke","name":"engine_request","arguments":{"method":"POST","path":"/api/v1/workspace"}}),
                RequestKind::Mutation,
            ),
        ] {
            let request: Request =
                serde_json::from_value(json!({"id":1,"command":command})).expect("request");
            assert_eq!(request.kind(), kind);
        }
    }

    #[tokio::test]
    async fn previous_session_and_unfenced_recorder_never_reach_engine() {
        // Never the session's real engine: a regression here must not reach live audio.
        let fixture = mixrelay_test_support::FixtureDir::new("ipc-fence").expect("fixture");
        let control = LocalControl::new(
            ControlProxyClient::with_socket(Some(fixture.join("no-engine.sock"))),
            #[cfg(target_os = "macos")]
            Arc::default(),
        )
        .expect("control");
        for (kind, session, request) in [
            (
                "node",
                "previous",
                json!({"expectedRevision":1,"enabled":false}),
            ),
            (
                "recorder",
                control.session.as_str(),
                json!({"expectedRevision":1,"request":{"action":"stop"}}),
            ),
        ] {
            let request = serde_json::from_value(json!({"id":9,"command":{"type":kind,"session":session,"target":{"graphId":"g","nodeId":"n"},"request":request}})).expect("request");
            let response = control.dispatch(request).await;
            assert_eq!(response["status"], 409);
            assert_eq!(
                response["body"]["parameters"]["nodeControlOutcome"],
                "not_applied"
            );
        }
    }
}
