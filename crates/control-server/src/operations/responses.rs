//! Stored operation results and the replies that describe an operation's outcome.
use axum::{
    Json,
    body::{Body, Bytes},
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};

/// Names the pending operation a client polls.
pub(super) const OPERATION_HEADER: &str = "x-mixrelay-operation";
/// Names the receipt whose original result a reply carries.
pub(super) const RESULT_HEADER: &str = "x-mixrelay-operation-result";

/// Response marker of a blocking task that was lost before it reported its outcome.
#[derive(Clone, Copy)]
pub(super) struct UncertainTask;

/// Marks a lost blocking task as uncertain so polling cannot confirm its mutation outcome.
pub(crate) fn task_failure(code: ErrorCode, reason: &'static str) -> Response {
    let mut response = (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ProtocolError {
            code,
            parameters: serde_json::json!({"operationOutcome": "uncertain", "reason": reason}),
        }),
    )
        .into_response();
    response.extensions_mut().insert(UncertainTask);
    response
}

/// A terminal response kept in a receipt until it is acknowledged or expires.
#[derive(Clone, Debug)]
pub(super) struct StoredResponse {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Bytes,
    /// False when the server replaced the handler's own reply with an uncertain outcome.
    pub(super) original: bool,
}

impl StoredResponse {
    /// Replays the stored reply. Only an original reply names the receipt it came from.
    pub(super) fn into_response(self, receipt_id: Option<&str>) -> Response {
        let mut response = Response::new(Body::from(self.body));
        *response.status_mut() = self.status;
        *response.headers_mut() = self.headers;
        if let Some(receipt_id) = receipt_id.filter(|_| self.original)
            && let Ok(value) = HeaderValue::try_from(receipt_id)
        {
            response
                .headers_mut()
                .insert(HeaderName::from_static(RESULT_HEADER), value);
        }
        response
    }
}

/// A stored outcome the server cannot vouch for: the work may or may not have applied.
pub(super) fn uncertain_response(reason: &str) -> StoredResponse {
    let error = ProtocolError {
        code: ErrorCode::ControlUnavailable,
        parameters: serde_json::json!({
            "operationOutcome": "uncertain",
            "reason": reason,
        }),
    };
    let body = serde_json::to_vec(&error)
        .unwrap_or_else(|_| b"{\"code\":\"control_unavailable\"}".to_vec());
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    StoredResponse {
        status: StatusCode::SERVICE_UNAVAILABLE,
        headers,
        body: Bytes::from(body),
        original: false,
    }
}

#[derive(serde::Serialize)]
struct PendingResponse<'a> {
    #[serde(rename = "operationId")]
    operation_id: &'a str,
}

pub(super) fn pending_response(id: &str) -> Response {
    let mut response = Json(PendingResponse { operation_id: id }).into_response();
    *response.status_mut() = StatusCode::ACCEPTED;
    add_operation_header(response.headers_mut(), id);
    response
}

pub(super) fn not_admitted(status: StatusCode, code: ErrorCode) -> Response {
    (
        status,
        Json(ProtocolError {
            code,
            parameters: serde_json::json!({"operationOutcome": "not_admitted"}),
        }),
    )
        .into_response()
}

/// Rejects work for lack of capacity. A mutation reports the outcome key of the
/// transaction it targeted, so clients know that transaction did not apply.
pub(crate) fn admission_full_response(request: &Request) -> Response {
    let mut parameters = serde_json::json!({});
    if request.method() != Method::GET {
        let outcome = match request.uri().path() {
            crate::GRAPH_CATALOG_ROUTE => "catalogOutcome",
            "/api/v1/workspace" | "/api/v1/workspace/preview" => "workspaceOutcome",
            "/api/v1/audio/power" | "/api/v1/audio/power/preview" => "powerOutcome",
            path if path.starts_with("/api/v1/graphs/") && path.contains("/nodes/") => {
                "nodeControlOutcome"
            }
            _ => "operationOutcome",
        };
        parameters[outcome] = serde_json::json!("not_applied");
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters,
        }),
    )
        .into_response()
}

pub(super) fn missing_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"operationOutcome": "uncertain"}),
        }),
    )
        .into_response()
}

pub(super) fn add_operation_header(headers: &mut HeaderMap, id: &str) {
    if let Ok(value) = HeaderValue::try_from(id) {
        headers.insert(HeaderName::from_static(OPERATION_HEADER), value);
    }
}

#[cfg(test)]
mod tests;
