//! Polling and acknowledging operation receipts (`/api/v1/operations/{id}`).
use axum::{
    Extension, Json,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};

use super::{
    ControlOperations,
    ledger::{Ack, Snapshot},
    responses::{add_operation_header, missing_response, pending_response},
};

pub(crate) async fn get(
    Extension(operations): Extension<ControlOperations>,
    Path(id): Path<String>,
) -> Response {
    match operations.snapshot(&id) {
        Snapshot::Pending => pending_response(&id),
        Snapshot::Complete(response) => response.into_response(Some(&id)),
        Snapshot::Missing => missing_response(),
    }
}

pub(crate) async fn delete(
    Extension(operations): Extension<ControlOperations>,
    Path(id): Path<String>,
) -> Response {
    match operations.acknowledge(&id) {
        Ack::Completed => StatusCode::NO_CONTENT.into_response(),
        Ack::Pending => {
            let mut response = (
                StatusCode::CONFLICT,
                Json(ProtocolError {
                    code: ErrorCode::ControlUnavailable,
                    parameters: serde_json::json!({"operationOutcome": "pending"}),
                }),
            )
                .into_response();
            add_operation_header(response.headers_mut(), &id);
            response
        }
        Ack::Missing => missing_response(),
    }
}
