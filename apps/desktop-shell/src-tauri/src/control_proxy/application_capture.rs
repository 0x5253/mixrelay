//! Desktop-owned taps follow the complete engine execution manifest, never UI selection.
//! Each capture-changing command previews its manifest, reserves the taps, submits, and
//! settles the reservation from the answer (`engine_control::settlement`).
use std::sync::{Arc, Weak, atomic::Ordering};
use std::time::Duration;

use mixrelay_control_protocol::{
    ApplicationCaptureManifest, GraphRuntimeStatus, NodeControlPreview,
};
use mixrelay_desktop_core::{
    engine_control::{
        routes,
        settlement::{self, CAPTURES_RETAINED, Settlement, Submission},
    },
    timing::AsyncDelay as _,
};
use mixrelay_time::{Deadline, MonotonicClock as _};

use super::{ControlProxyClient, ControlProxyResponse, Method, Value};
use crate::async_time::TokioDelay;
use crate::macos_audio::{ApplicationCaptureState, PreparedApplicationCaptures};

/// Monitor cadence while waiting for the engine to acknowledge staged captures.
const ACKNOWLEDGEMENT_POLL: Duration = Duration::from_millis(100);
/// Monitor cadence after an acknowledgement attempt failed.
const ACKNOWLEDGEMENT_RETRY: Duration = Duration::from_secs(1);
const ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a capture-changing submission goes.
struct Target {
    submission: Submission,
    method: Method,
    route: String,
}

impl Target {
    fn put(submission: Submission, route: impl Into<String>) -> Self {
        Self {
            submission,
            method: Method::PUT,
            route: route.into(),
        }
    }
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
            &routes::preview(&route),
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
    let target = Target::put(Submission::NodeControl, route);
    submit_prepared(client, captures, request, manifest, target).await
}

pub(super) async fn enable(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
) -> Result<ControlProxyResponse, String> {
    let preview = client
        .request(Method::POST, routes::AUDIO_POWER_PREVIEW, None)
        .await?;
    if !preview.is_success() {
        return Ok(preview);
    }
    let manifest = decode_manifest(preview.body)?;
    let expected = manifest
        .revision
        .checked_sub(1)
        .ok_or("invalid engine power preflight revision")?;
    let request = serde_json::json!({"enabled":true,"expectedRevision":expected});
    let target = Target::put(Submission::Power, routes::AUDIO_POWER);
    submit_prepared(client, captures, request, manifest, target).await
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
        .request(Method::GET, routes::GRAPH_STATUS, None)
        .await?;
    if !graph.is_success() {
        return Ok(graph);
    }
    let graph: GraphRuntimeStatus = serde_json::from_value(graph.body)
        .map_err(|error| format!("invalid engine stop acknowledgement: {error}"))?;
    if captures.requires_manifest(graph.submitted_revision)? {
        let manifest = client
            .request(Method::GET, routes::CAPTURE_MANIFEST, None)
            .await?;
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

/// After a successful read or write, stages the captures the engine's manifest now needs.
pub(super) async fn synchronize_response(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    response: ControlProxyResponse,
) -> Result<ControlProxyResponse, String> {
    if !response.is_success() {
        return Ok(response);
    }
    let manifest = client
        .request(Method::GET, routes::CAPTURE_MANIFEST, None)
        .await?;
    if !manifest.is_success() {
        return Ok(manifest);
    }
    let manifest = decode_manifest(manifest.body)?;
    let prepared = match prepare(captures, manifest).await {
        Ok(prepared) => prepared,
        Err(error) => return Ok(ControlProxyResponse::capture_unavailable(&error)),
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
        .request(
            Method::POST,
            routes::CAPTURE_PREVIEW,
            Some(document.clone()),
        )
        .await?;
    if !preview.is_success() {
        return Ok(preview);
    }
    let manifest = decode_manifest(preview.body)?;
    let target = Target::put(Submission::Graph, routes::GRAPH);
    submit_prepared(client, captures, document, manifest, target).await
}

pub(super) async fn workspace(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    mut request: Value,
) -> Result<ControlProxyResponse, String> {
    let preview = client
        .request(
            Method::POST,
            routes::WORKSPACE_PREVIEW,
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
    let target = Target {
        submission: Submission::Workspace,
        method: Method::POST,
        route: routes::WORKSPACE.to_owned(),
    };
    submit_prepared(client, captures, request, preview.manifest, target).await
}

/// Reserves the manifest's taps, submits, then releases, retains or accepts them.
async fn submit_prepared(
    client: &ControlProxyClient,
    captures: &Arc<ApplicationCaptureState>,
    document: Value,
    manifest: ApplicationCaptureManifest,
    target: Target,
) -> Result<ControlProxyResponse, String> {
    let revision = manifest.revision;
    if let Err(error) = reserve(captures, manifest).await {
        return Ok(ControlProxyResponse::capture_unavailable(&error));
    }
    // A cancelled command future or lost PUT response cannot destroy its taps:
    // the reservation belongs to managed state before the request starts.
    start_monitor(client, captures);
    let mut response = client
        .request(target.method, &target.route, Some(document))
        .await
        .map_err(|error| format!("{error}; {CAPTURES_RETAINED}"))?;
    let settlement = settlement::settle(
        target.submission,
        response.delivery,
        response.status,
        &response.body,
        revision,
    )
    .map_err(|error| format!("{error}; {CAPTURES_RETAINED}"))?;
    match settlement {
        Settlement::Release => reject_reserved(captures).await?,
        Settlement::Retain => settlement::mark_pending_confirmation(&mut response.body),
        Settlement::Accept => accept_reserved(captures).await?,
    }
    Ok(response)
}

fn decode_manifest(body: Value) -> Result<ApplicationCaptureManifest, String> {
    serde_json::from_value(body)
        .map_err(|error| format!("invalid application capture manifest: {error}"))
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
        let timer = TokioDelay::new(mixrelay_time::StdClock::new());
        tauri::async_runtime::spawn(monitor(client.clone(), Arc::downgrade(captures), timer));
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

/// Observes engine acknowledgements until no staged or reserved capture is pending.
async fn monitor(
    client: ControlProxyClient,
    weak: Weak<ApplicationCaptureState>,
    timer: TokioDelay,
) {
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
        let interval = if error.is_some() {
            ACKNOWLEDGEMENT_RETRY
        } else {
            ACKNOWLEDGEMENT_POLL
        };
        last_error = error;
        timer
            .wait_until(Deadline::after(timer.now(), interval))
            .await;
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
        ACKNOWLEDGEMENT_TIMEOUT,
        client.request(Method::GET, routes::GRAPH_STATUS, None),
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
        let manifest = client
            .request(Method::GET, routes::CAPTURE_MANIFEST, None)
            .await?;
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
        let response = ControlProxyResponse::answered(
            None,
            200,
            serde_json::json!({"enginePower":{"enabled":false,"transitioning":false}}),
        );

        let rejected = power_status(&client, &captures, response).await.unwrap();
        assert_eq!(rejected.status, 409);
        assert_eq!(rejected.body["code"], "control_unavailable");
        assert_eq!(
            rejected.delivery,
            mixrelay_desktop_core::engine_control::Delivery::NotSent
        );
        assert!(!captures.has_pending().unwrap());
    }

    #[tokio::test]
    async fn a_refused_preview_is_returned_without_reserving_taps() {
        let client = ControlProxyClient::with_socket(None);
        let captures = Arc::new(ApplicationCaptureState::default());
        let refused = apply(&client, &captures, serde_json::json!({"revision":1}))
            .await
            .unwrap();
        assert_eq!(refused.status, 409);
        assert!(!captures.has_pending().unwrap());
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
