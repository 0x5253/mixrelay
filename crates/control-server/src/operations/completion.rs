//! Turning a detached handler's reply into a receipt, and waiting for it briefly.
use std::sync::Arc;

use axum::{body::to_bytes, http::HeaderName, response::Response};
use tokio::sync::Notify;

use super::{
    ControlOperations,
    ledger::Snapshot,
    responses::{
        OPERATION_HEADER, RESULT_HEADER, StoredResponse, UncertainTask, uncertain_response,
    },
};

/// The largest handler reply a receipt keeps.
pub(super) const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

/// Stores a handler's reply without operation headers, which the server owns.
/// A reply that cannot be stored in full becomes an uncertain outcome.
pub(super) async fn collect_response(response: Response) -> StoredResponse {
    let (parts, body) = response.into_parts();
    let mut headers = parts.headers;
    headers.remove(HeaderName::from_static(OPERATION_HEADER));
    headers.remove(HeaderName::from_static(RESULT_HEADER));
    match to_bytes(body, RESPONSE_LIMIT.saturating_add(1)).await {
        Ok(body) if body.len() <= RESPONSE_LIMIT => StoredResponse {
            status: parts.status,
            headers,
            body,
            original: parts.extensions.get::<UncertainTask>().is_none(),
        },
        Ok(_) => uncertain_response("operation response exceeded bounded storage"),
        Err(error) => uncertain_response(&format!("operation response failed: {error}")),
    }
}

/// Waits up to the completion wait and reports whether the operation completed.
pub(super) async fn completion_after_wait(
    operations: &ControlOperations,
    id: &str,
    notify: Arc<Notify>,
) -> bool {
    let notified = notify.notified();
    if matches!(operations.snapshot(id), Snapshot::Complete(_)) {
        return true;
    }
    let _ = tokio::time::timeout(operations.timings().completion_wait, notified).await;
    matches!(operations.snapshot(id), Snapshot::Complete(_))
}

/// Runs a handler in its own task, so a panic leaves an uncertain receipt instead of none.
pub(super) async fn run_to_receipt(
    handler: impl Future<Output = Response> + Send + 'static,
) -> StoredResponse {
    match tokio::spawn(handler).await {
        Ok(response) => collect_response(response).await,
        Err(error) => uncertain_response(&format!("operation task failed: {error}")),
    }
}

#[cfg(test)]
mod tests;
