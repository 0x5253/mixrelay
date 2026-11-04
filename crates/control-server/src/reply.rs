//! Replies shared by the control handlers: protocol error statuses, undecodable bodies
//! and backend calls that must not block Tokio's I/O workers.
use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};
use serde::Serialize;

use crate::operations::task_failure;

pub(crate) fn protocol_error_status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidOperationId => StatusCode::BAD_REQUEST,
        ErrorCode::GraphRevisionStale
        | ErrorCode::GraphExchangeBusy
        | ErrorCode::NodeDisabled
        | ErrorCode::EngineReplaced => StatusCode::CONFLICT,
        ErrorCode::NodeNotFound => StatusCode::NOT_FOUND,
        ErrorCode::GraphInvalid
        | ErrorCode::GraphBindingUnsupported
        | ErrorCode::AudioSessionUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Unauthorized | ErrorCode::PairingRequired => StatusCode::UNAUTHORIZED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Replies with a backend error under its protocol status.
pub(crate) fn failure(error: ProtocolError) -> Response {
    (protocol_error_status(error.code), Json(error)).into_response()
}

/// Replies with `status` and the value, or with the error under its protocol status.
pub(crate) fn reply<T: Serialize>(
    status: StatusCode,
    result: Result<T, ProtocolError>,
) -> Response {
    match result {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => failure(error),
    }
}

/// Rejects a body that does not decode before it reaches the backend.
pub(crate) fn unprocessable(code: ErrorCode, rejection: &JsonRejection) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ProtocolError {
            code,
            parameters: serde_json::json!({"reason": rejection.body_text()}),
        }),
    )
        .into_response()
}

// Catalog reads may wait behind a native snapshot and writes perform filesystem I/O.
// Neither may block a Tokio reactor, even when there is only one asynchronous worker.
pub(crate) async fn catalog_task(work: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|_| {
        task_failure(
            ErrorCode::ControlUnavailable,
            "graph catalog control task failed",
        )
    })
}

/// Runs a backend call that may block (driver negotiation, plug-in preparation, durable
/// writes) on the blocking pool and replies with its result under `status`. A lost task
/// is an uncertain outcome reported as `failed_code` with `failed_reason`.
pub(crate) async fn blocking_reply<T: Serialize + Send + 'static>(
    status: StatusCode,
    failed_code: ErrorCode,
    failed_reason: &'static str,
    work: impl FnOnce() -> Result<T, ProtocolError> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => reply(status, result),
        Err(_) => task_failure(failed_code, failed_reason),
    }
}

#[cfg(test)]
mod tests;
