//! Routers, requests and assertions shared by the operation tests.
use std::time::Duration;

use axum::{
    Extension, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Path, Request},
    http::{HeaderMap, StatusCode},
    middleware,
    response::Response,
    routing::{MethodRouter, get},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError};
use serde_json::Value;

use super::{
    ControlOperations, OperationTimings, dispatch,
    ledger::Snapshot,
    receipt_routes,
    responses::{OPERATION_HEADER, StoredResponse},
};

/// Long enough for an immediate handler to complete inside the window on a loaded runner.
pub(super) const COMPLETION_WAIT: Duration = Duration::from_millis(25);

pub(super) fn fast_operations() -> ControlOperations {
    operations_waiting(COMPLETION_WAIT)
}

/// An owner whose callers wait `completion_wait` before a reply becomes pending.
pub(super) fn operations_waiting(completion_wait: Duration) -> ControlOperations {
    ControlOperations::new(crate::test_support::clock()).with_timings(OperationTimings {
        completion_wait,
        ..OperationTimings::default()
    })
}

/// Serves `work` at `/work` behind the dispatcher, with the receipt routes beside it.
pub(super) fn test_router(operations: ControlOperations, work: MethodRouter) -> Router {
    Router::new()
        .route("/work", work)
        .route(
            "/api/v1/operations/{id}",
            get(receipt_routes::get).delete(receipt_routes::delete),
        )
        .layer(middleware::from_fn(dispatch::dispatch))
        .layer(Extension(operations))
}

pub(super) fn post_work() -> Request {
    Request::post("/work").body(Body::empty()).expect("request")
}

pub(super) fn post_work_as(id: impl AsRef<str>) -> Request {
    Request::post("/work")
        .header(dispatch::REQUEST_HEADER, id.as_ref())
        .body(Body::empty())
        .expect("request")
}

/// Asserts a pending reply and returns the operation ID it names.
pub(super) async fn pending_id(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let header = response
        .headers()
        .get(OPERATION_HEADER)
        .expect("pending operation header")
        .to_str()
        .expect("operation id header")
        .to_owned();
    let body = to_bytes(response.into_body(), 4096)
        .await
        .expect("pending response body");
    let body: Value = serde_json::from_slice(&body).expect("pending JSON");
    assert_eq!(body["operationId"], header);
    header
}

/// Polls until the operation completes, then replies as the receipt route does.
pub(super) async fn terminal_response(operations: &ControlOperations, id: &str) -> Response {
    for _ in 0..200 {
        if matches!(operations.snapshot(id), Snapshot::Complete(_)) {
            return receipt_routes::get(Extension(operations.clone()), Path(id.to_owned())).await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("operation did not complete");
}

pub(super) async fn json_body(response: Response) -> Value {
    let body = to_bytes(response.into_body(), 4096).await.expect("body");
    serde_json::from_slice(&body).expect("JSON")
}

pub(super) fn stored(status: StatusCode, body: &'static [u8], original: bool) -> StoredResponse {
    StoredResponse {
        status,
        headers: HeaderMap::new(),
        body: Bytes::from_static(body),
        original,
    }
}

pub(super) async fn assert_invalid_id(response: Response) {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let error: ProtocolError = serde_json::from_slice(&body).unwrap();
    assert_eq!(error.code, ErrorCode::InvalidOperationId);
    assert_eq!(error.parameters["operationOutcome"], "not_admitted");
}
