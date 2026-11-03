//! Desktop-owned taps follow the complete engine execution manifest, never UI selection.
use std::sync::{Arc, Weak, atomic::Ordering};
use std::time::Duration;

use mixrelay_control_protocol::{
    ApplicationCaptureManifest, GraphRuntimeStatus, NodeControlPreview,
};

use super::{
    ControlProxyClient, ControlProxyResponse, Method, RequestDelivery, Value,
    application_capture_error,
};
use crate::macos_audio::{ApplicationCaptureState, PreparedApplicationCaptures};

const MANIFEST: &str = "/api/v1/application-capture/manifest";
const PREVIEW: &str = "/api/v1/application-capture/preview";

#[derive(PartialEq, Eq)]
enum Submission {
    Graph,
    Workspace,
    Power,
    NodeControl(String),
}

pub(super) async fn node_control(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    route: String,
    request: Value,
) -> Result<ControlProxyResponse, String> {
    let preview = client
        .request(
            Method::POST,
            &format!("{route}/preview"),
            Some(request.clone()),
        )
        .await?;
    if !preview.is_success() {
        return Ok(preview);
    }
    let preview: NodeControlPreview = serde_json::from_value(preview.body)
        .map_err(|error| format!("invalid node control preview: {error}"))?;
    let Some(manifest) = preview.capture_manifest else {
        return client.request(Method::PUT, &route, Some(request)).await;
    };
    submit_prepared(
        client,
        captures,
        request,
        manifest,
        Submission::NodeControl(route),
    )
    .await
}

pub(super) async fn enable(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
) -> Result<ControlProxyResponse, String> {
    let preview = client
        .request(Method::POST, "/api/v1/audio/power/preview", None)
        .await?;
    if !preview.is_success() {
        return Ok(preview);
    }
    let manifest = decode_manifest(preview.body)?;
    let expected = manifest
        .revision
        .checked_sub(1)
        .ok_or("invalid engine power preflight revision")?;
    submit_prepared(
        client,
        captures,
        serde_json::json!({"enabled":true,"expectedRevision":expected}),
        manifest,
        Submission::Power,
    )
    .await
}

pub(super) async fn power_status(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    mut response: ControlProxyResponse,
) -> Result<ControlProxyResponse, String> {
    if !response.is_success() || response.body["enginePower"]["enabled"] != false {
        return Ok(response);
    }
    // Also repairs a lost stop-command reply. The authoritative empty manifest,
    // not UI intent, authorizes releasing desktop-owned capture sessions.
    let _guard = client.capture_gate.lock().await;
    let graph = client
        .request(Method::GET, "/api/v1/graph/status", None)
        .await?;
    if !graph.is_success() {
        return Ok(graph);
    }
    let graph: GraphRuntimeStatus = serde_json::from_value(graph.body)
        .map_err(|error| format!("invalid engine stop acknowledgement: {error}"))?;
    if captures.requires_manifest(graph.submitted_revision)? {
        let manifest = client.request(Method::GET, MANIFEST, None).await?;
        if !manifest.is_success() {
            return Ok(manifest);
        }
        let manifest = decode_manifest(manifest.body)?;
        // Power may have changed again since the status response was read.
        if !manifest.sources.is_empty() {
            response.body["enginePower"]["transitioning"] = Value::Bool(true);
            return Ok(response);
        }
        stage_owner(captures, prepare(captures, manifest).await?).await?;
    }
    let owner = Arc::clone(captures);
    tauri::async_runtime::spawn_blocking(move || owner.acknowledge(graph.active_revision))
        .await
        .map_err(|error| error.to_string())??;
    if captures.has_pending()? || mixrelay_macos_process_tap::active_capture_owners() != 0 {
        response.body["enginePower"]["transitioning"] = Value::Bool(true);
    }
    Ok(response)
}

pub(super) async fn synchronize_response(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    response: ControlProxyResponse,
) -> Result<ControlProxyResponse, String> {
    if !response.is_success() {
        return Ok(response);
    }
    let manifest = client.request(Method::GET, MANIFEST, None).await?;
    if !manifest.is_success() {
        return Ok(manifest);
    }
    let manifest = decode_manifest(manifest.body)?;
    let prepared = match prepare(captures, manifest).await {
        Ok(prepared) => prepared,
        Err(error) => return Ok(application_capture_error(&error)),
    };
    stage(client, captures, prepared).await?;
    Ok(response)
}

pub(super) async fn apply(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    document: Value,
) -> Result<ControlProxyResponse, String> {
    let preview = client
        .request(Method::POST, PREVIEW, Some(document.clone()))
        .await?;
    if !preview.is_success() {
        return Ok(preview);
    }
    let manifest = decode_manifest(preview.body)?;
    submit_prepared(client, captures, document, manifest, Submission::Graph).await
}

pub(super) async fn workspace(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    mut request: Value,
) -> Result<ControlProxyResponse, String> {
    let preview = client
        .request(
            Method::POST,
            "/api/v1/workspace/preview",
            Some(request.clone()),
        )
        .await?;
    if !preview.is_success() {
        return Ok(preview);
    }
    let preview: mixrelay_control_protocol::WorkspacePreview = serde_json::from_value(preview.body)
        .map_err(|error| format!("invalid workspace capture preview: {error}"))?;
    request["fileDigest"] =
        serde_json::to_value(preview.file_digest).map_err(|error| error.to_string())?;
    submit_prepared(
        client,
        captures,
        request,
        preview.manifest,
        Submission::Workspace,
    )
    .await
}

async fn submit_prepared(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    document: Value,
    manifest: ApplicationCaptureManifest,
    submission: Submission,
) -> Result<ControlProxyResponse, String> {
    let revision = manifest.revision;
    if let Err(error) = reserve(captures, manifest).await {
        return Ok(application_capture_error(&error));
    }
    // A cancelled command future or lost PUT response cannot destroy its taps:
    // the reservation belongs to managed state before the request starts.
    start_monitor(client, captures);
    let result = client
        .request(
            if submission == Submission::Workspace {
                Method::POST
            } else {
                Method::PUT
            },
            match &submission {
                Submission::Workspace => "/api/v1/workspace",
                Submission::Graph => "/api/v1/graph",
                Submission::Power => "/api/v1/audio/power",
                Submission::NodeControl(route) => route,
            },
            Some(document),
        )
        .await;
    if matches!(&result, Ok(response) if definitively_rejected(response)) {
        reject_reserved(captures).await?;
        return result;
    }
    if !matches!(&result, Ok(response) if response.is_success()) {
        return unresolved_result(result);
    }
    let response = result?;
    let acknowledgement = match submission {
        Submission::Workspace => &response.body["document"],
        Submission::NodeControl(_) => &response.body["application"],
        Submission::Graph | Submission::Power => &response.body,
    };
    validate_ack(acknowledgement, revision).map_err(|error| {
        format!("{error}; application captures retained until engine confirmation")
    })?;
    accept_reserved(captures).await?;
    Ok(response)
}

fn unresolved_result(
    result: Result<ControlProxyResponse, String>,
) -> Result<ControlProxyResponse, String> {
    match result {
        Ok(mut response) => {
            if let Some(parameters) = response
                .body
                .get_mut("parameters")
                .and_then(Value::as_object_mut)
            {
                parameters.insert(
                    "captureOutcome".into(),
                    Value::String("pending_confirmation".into()),
                );
            }
            Ok(response)
        }
        Err(error) => Err(format!(
            "{error}; application captures retained until engine confirmation"
        )),
    }
}

fn definitively_rejected(response: &ControlProxyResponse) -> bool {
    response.delivery == RequestDelivery::NotSent
        || (response.delivery == RequestDelivery::ServerResponse
            && ((400..500).contains(&response.status)
                || response.body["parameters"]["workspaceOutcome"] == "not_applied"
                || response.body["parameters"]["powerOutcome"] == "not_applied"
                || response.body["parameters"]["nodeControlOutcome"] == "not_applied"))
}

fn decode_manifest(body: Value) -> Result<ApplicationCaptureManifest, String> {
    serde_json::from_value(body)
        .map_err(|error| format!("invalid application capture manifest: {error}"))
}

fn validate_ack(body: &Value, revision: u64) -> Result<(), String> {
    if body.get("revision").and_then(Value::as_u64) != Some(revision) {
        return Err("graph acceptance does not match prepared capture revision".into());
    }
    Ok(())
}

async fn prepare(
    captures: &Arc<ApplicationCaptureState>,
    manifest: ApplicationCaptureManifest,
) -> Result<PreparedApplicationCaptures, String> {
    let captures = Arc::clone(captures);
    tauri::async_runtime::spawn_blocking(move || captures.prepare(&manifest))
        .await
        .map_err(|error| format!("capture preparation task failed: {error}"))?
}

async fn reserve(
    captures: &Arc<ApplicationCaptureState>,
    manifest: ApplicationCaptureManifest,
) -> Result<(), String> {
    let captures = Arc::clone(captures);
    tauri::async_runtime::spawn_blocking(move || captures.reserve(&manifest))
        .await
        .map_err(|error| format!("capture reservation task failed: {error}"))?
}

async fn reject_reserved(captures: &Arc<ApplicationCaptureState>) -> Result<(), String> {
    let captures = Arc::clone(captures);
    tauri::async_runtime::spawn_blocking(move || captures.reject_reserved())
        .await
        .map_err(|error| format!("capture rejection task failed: {error}"))?
}

async fn accept_reserved(captures: &Arc<ApplicationCaptureState>) -> Result<(), String> {
    let captures = Arc::clone(captures);
    tauri::async_runtime::spawn_blocking(move || captures.accept_reserved())
        .await
        .map_err(|error| format!("capture acceptance task failed: {error}"))?
}

async fn stage(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    prepared: PreparedApplicationCaptures,
) -> Result<(), String> {
    stage_owner(captures, prepared).await?;
    start_monitor(client, captures);
    Ok(())
}

fn start_monitor(client: &ControlProxyClient, captures: &Arc<ApplicationCaptureState>) {
    // The caller owns capture_gate. The monitor clears this flag under the same
    // gate, so a new stage cannot lose its wake-up while the previous monitor exits.
    if !captures.monitor_active.swap(true, Ordering::AcqRel) {
        tauri::async_runtime::spawn(monitor(client.clone(), Arc::downgrade(captures)));
    }
}

async fn stage_owner(
    captures: &Arc<ApplicationCaptureState>,
    prepared: PreparedApplicationCaptures,
) -> Result<(), String> {
    let owner = Arc::clone(captures);
    tauri::async_runtime::spawn_blocking(move || owner.stage(prepared))
        .await
        .map_err(|error| format!("capture staging task failed: {error}"))?
}

async fn monitor(client: ControlProxyClient, weak: Weak<ApplicationCaptureState>) {
    let mut last_error = None;
    loop {
        {
            let _guard = client.capture_gate.lock().await;
            let Some(captures) = weak.upgrade() else {
                return;
            };
            if !captures.has_pending().unwrap_or(true) {
                captures.monitor_active.store(false, Ordering::Release);
                return;
            }
        }
        let error = observe_ack(&client, &weak).await.err();
        if error != last_error
            && let Some(error) = &error
        {
            tracing::warn!(%error, "application capture activation is awaiting engine acknowledgement");
        }
        let interval = if error.is_some() { 1000 } else { 100 };
        last_error = error;
        tokio::time::sleep(Duration::from_millis(interval)).await;
    }
}

async fn observe_ack(
    client: &ControlProxyClient,
    weak: &Weak<ApplicationCaptureState>,
) -> Result<(), String> {
    let Some(generation) = weak.upgrade().map(|captures| captures.engine_generation()) else {
        return Ok(());
    };
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        client.request(Method::GET, "/api/v1/graph/status", None),
    )
    .await
    .map_err(|_| "capture acknowledgement request timed out".to_owned())??;
    if !response.is_success() {
        return Err(format!("capture acknowledgement HTTP {}", response.status));
    }
    let status: GraphRuntimeStatus = serde_json::from_value(response.body)
        .map_err(|error| format!("invalid capture acknowledgement: {error}"))?;
    let _guard = client.capture_gate.lock().await;
    // A status read before the engine was lost cannot acknowledge its successor's captures.
    let Some(captures) = weak
        .upgrade()
        .filter(|captures| captures.engine_generation() == generation)
    else {
        return Ok(());
    };
    if captures.requires_manifest(status.submitted_revision)? {
        // Another controller may have advanced execution before this ACK. Its
        // authoritative manifest supersedes stale prepared sources, not UI state.
        let manifest = client.request(Method::GET, MANIFEST, None).await?;
        if !manifest.is_success() {
            return Err(format!("capture manifest HTTP {}", manifest.status));
        }
        let prepared = prepare(&captures, decode_manifest(manifest.body)?).await?;
        stage_owner(&captures, prepared).await?;
    }
    tauri::async_runtime::spawn_blocking(move || captures.acknowledge(status.active_revision))
        .await
        .map_err(|error| format!("capture acknowledgement task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_status_preserves_rejected_acknowledgement_without_decoding_or_staging() {
        let client = ControlProxyClient::with_socket(None);
        client.close_admission();
        let captures = Arc::new(ApplicationCaptureState::default());
        let response = ControlProxyResponse {
            engine: None,
            status: 200,
            delivery: RequestDelivery::ServerResponse,
            body: serde_json::json!({"enginePower":{"enabled":false,"transitioning":false}}),
        };

        let rejected = power_status(&client, &captures, response).await.unwrap();
        assert_eq!(rejected.status, 409);
        assert_eq!(rejected.body["code"], "control_unavailable");
        assert_eq!(rejected.delivery, RequestDelivery::NotSent);
        assert!(!captures.has_pending().unwrap());
    }

    #[test]
    fn explicit_workspace_rejection_releases_reservation_but_lost_reply_does_not() {
        let mut response = ControlProxyResponse {
            engine: None,
            status: 503,
            delivery: RequestDelivery::ServerResponse,
            body: serde_json::json!({"code":"control_unavailable", "parameters":{"workspaceOutcome":"not_applied"}}),
        };
        assert!(definitively_rejected(&response));
        response.delivery = RequestDelivery::Uncertain;
        assert!(!definitively_rejected(&response));
        response.delivery = RequestDelivery::ServerResponse;
        response.body = serde_json::json!({"code":"control_unavailable"});
        assert!(!definitively_rejected(&response));
    }

    #[test]
    fn acceptance_must_match_prepared_revision_exactly() {
        for revision in [0, 6, 8] {
            assert!(validate_ack(&serde_json::json!({"revision":revision}), 7).is_err());
        }
        assert!(validate_ack(&serde_json::json!({"revision":7}), 7).is_ok());
        assert!(validate_ack(&serde_json::json!({}), 7).is_err());
    }

    #[test]
    fn rejected_power_start_releases_reserved_taps_but_uncertain_reply_does_not() {
        let mut response = ControlProxyResponse {
            engine: None,
            status: 503,
            delivery: RequestDelivery::ServerResponse,
            body: serde_json::json!({"code":"audio_session_reconfigure_failed", "parameters":{"powerOutcome":"not_applied"}}),
        };
        assert!(definitively_rejected(&response));
        response.delivery = RequestDelivery::Uncertain;
        assert!(!definitively_rejected(&response));
    }

    #[test]
    fn rejected_node_control_releases_reservation_but_lost_response_does_not() {
        let mut response = ControlProxyResponse {
            engine: None,
            status: 503,
            delivery: RequestDelivery::ServerResponse,
            body: serde_json::json!({"code":"node_disabled", "parameters":{"nodeControlOutcome":"not_applied"}}),
        };
        assert!(definitively_rejected(&response));
        response.delivery = RequestDelivery::Uncertain;
        assert!(!definitively_rejected(&response));
    }

    #[test]
    fn authored_document_is_not_a_capture_manifest() {
        assert!(
            decode_manifest(serde_json::json!({
                "schemaVersion":2,"revision":7,"nodes":[],"edges":[]
            }))
            .is_err()
        );
    }
}
