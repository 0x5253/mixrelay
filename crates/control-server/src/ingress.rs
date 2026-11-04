use crate::{ControlOperations, operations::IngressLease};
use axum::{
    Json,
    body::{Body, HttpBody, to_bytes},
    extract::{Extension, Request},
    http::{HeaderValue, StatusCode, header::CONNECTION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};

const BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Bound ingress before handing ownership to a mutation. Once dispatched, a native
/// transaction or recorder finalization must complete even while the server shuts down.
pub(super) async fn read_body(
    Extension(operations): Extension<ControlOperations>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(mut response) = crate::operations::before_ingress(&operations, &request) {
        // The body was deliberately not drained; do not reuse this HTTP connection.
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
        return response;
    }
    let (mut parts, body) = request.into_parts();
    let lease = if body.is_end_stream() {
        None
    } else if let Some(lease) = operations.try_acquire_ingress() {
        Some(lease)
    } else {
        let mut response =
            crate::operations::admission_full_response(&Request::from_parts(parts, body));
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
        return response;
    };
    let bytes = match tokio::time::timeout(
        operations.timings().body_deadline,
        to_bytes(body, BODY_LIMIT),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => return rejected(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(_) => {
            return rejected(
                StatusCode::REQUEST_TIMEOUT,
                "request body deadline exceeded",
            );
        }
    };
    if let Some(lease_for_request) = lease.clone() {
        parts.extensions.insert::<IngressLease>(lease_for_request);
    }
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    drop(lease);
    response
}

fn rejected(status: StatusCode, reason: &str) -> Response {
    (
        status,
        [(CONNECTION, "close")],
        Json(ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({"reason": reason}),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
