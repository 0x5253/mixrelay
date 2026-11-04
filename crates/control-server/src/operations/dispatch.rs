//! Which requests become operations, and the middleware that admits and detaches them.
use axum::{
    Extension,
    extract::{Path, Request},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use mixrelay_control_protocol::ErrorCode;

use super::{
    ControlOperations, IngressLease,
    completion::{completion_after_wait, run_to_receipt},
    ledger::{Rejection, Snapshot},
    receipt_routes,
    responses::{admission_full_response, not_admitted, pending_response},
};

/// Carries a client-chosen operation ID, so a retried request never runs twice.
pub(super) const REQUEST_HEADER: &str = "x-mixrelay-operation-request";

/// Resolve retained identities before touching the body or its admission budget.
/// Dispatch still checks atomically when admitting a new identity after ingress.
pub(crate) fn before_ingress(
    operations: &ControlOperations,
    request: &Request,
) -> Option<Response> {
    if !needs_operation(request) {
        return None;
    }
    let id = match client_operation_id(request) {
        Ok(id) => id?,
        Err(code) => return Some(not_admitted(StatusCode::BAD_REQUEST, code)),
    };
    match operations.snapshot(id) {
        Snapshot::Pending => Some(pending_response(id)),
        Snapshot::Complete(response) => Some(response.into_response(Some(id))),
        Snapshot::Missing => None,
    }
}

/// Admits an operation request under a receipt and runs its handler detached from the
/// caller. A reply within the completion wait is returned directly; a slower one
/// becomes a pending operation the client polls.
pub(crate) async fn dispatch(
    Extension(operations): Extension<ControlOperations>,
    mut request: Request,
    next: Next,
) -> Response {
    let lease = request.extensions_mut().remove::<IngressLease>();
    if !needs_operation(&request) {
        let response = next.run(request).await;
        drop(lease);
        return response;
    }
    let requested = match client_operation_id(&request) {
        Ok(id) => id.map(str::to_owned),
        Err(code) => return not_admitted(StatusCode::BAD_REQUEST, code),
    };
    let admission = match operations.admit(requested.as_deref()) {
        Ok(admission) => admission,
        Err(Rejection::Pending(id)) => return pending_response(id),
        Err(Rejection::Complete(id, response)) => return response.into_response(Some(id)),
        Err(Rejection::Full) => return admission_full_response(&request),
    };
    let operation_id = admission.id.clone();
    let task_operations = operations.clone();
    tokio::spawn(async move {
        let _lease = lease;
        let response = run_to_receipt(next.run(request)).await;
        task_operations.complete(&operation_id, response);
    });

    let completed = completion_after_wait(&operations, &admission.id, admission.notify).await;
    if completed && requested.is_some() {
        return receipt_routes::get(Extension(operations), Path(admission.id)).await;
    }
    if completed && let Some(response) = operations.take_completed(&admission.id) {
        return response.into_response(None);
    }
    pending_response(&admission.id)
}

/// Whether a request is a tracked operation: every mutation, and reads that may wait on
/// native state. Operation polling, health, status, meters and editor frames never are.
pub(super) fn needs_operation(request: &Request) -> bool {
    let path = request.uri().path();
    if path.starts_with("/api/v1/operations/") || path == "/api/v1/operations" {
        return false;
    }
    if path == crate::HEALTH_ROUTE
        || path == crate::STATUS_ROUTE
        || path == crate::GRAPH_STATUS_ROUTE
        || path == crate::CONTROL_SNAPSHOT_ROUTE
        || path == crate::METERS_ROUTE
        || path.ends_with("/editor/frame")
        || path.ends_with("/editor/input")
    {
        return false;
    }
    if request.method() == Method::GET {
        return path == crate::GRAPH_ROUTE
            || path == crate::GRAPH_CATALOG_ROUTE
            || path.starts_with("/api/v1/graphs/")
            || path == crate::PROCESSORS_ROUTE
            || path == crate::APPLICATION_CAPTURE_MANIFEST_ROUTE;
    }
    request.method() == Method::POST || request.method() == Method::PUT
}

fn client_operation_id(request: &Request) -> Result<Option<&str>, ErrorCode> {
    match request.headers().get(REQUEST_HEADER) {
        None => Ok(None),
        Some(header) => match header.to_str() {
            Ok(id) if valid_client_id(id) => Ok(Some(id)),
            _ => Err(ErrorCode::InvalidOperationId),
        },
    }
}

fn valid_client_id(id: &str) -> bool {
    (16..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

#[cfg(test)]
mod tests;
