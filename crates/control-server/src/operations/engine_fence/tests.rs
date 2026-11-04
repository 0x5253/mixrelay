use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderValue, Method, Request, StatusCode, header::CONTENT_TYPE},
    middleware,
    routing::post,
};
use mixrelay_control_protocol::{
    ErrorCode, GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION, ProtocolError,
};
use tower::ServiceExt;

use super::{ENGINE_HEADER, engine_identity};
use crate::{
    ControlOperations, GRAPH_ROUTE, HEALTH_ROUTE,
    operations::{
        dispatch::REQUEST_HEADER,
        test_support::{fast_operations, test_router},
    },
    router_with_backend,
    test_support::{TestBackend, clock, document},
};

#[tokio::test]
async fn engine_header_and_fence_cover_success_errors_and_fallbacks() {
    let mut backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        persistence_error: None,
        revision: 2,
        state: GraphApplyState::Pending,
    }));
    Arc::get_mut(&mut backend).unwrap().incarnation = "engine-one".into();
    let operations = ControlOperations::new(clock());
    let router = router_with_backend(backend.clone(), operations);
    for expected in [Some("old-engine"), Some("engine-one"), None] {
        *backend.last_apply.lock().unwrap() = None;
        let mut request = Request::put(GRAPH_ROUTE).header(CONTENT_TYPE, "application/json");
        if let Some(expected) = expected {
            request = request.header("x-mixrelay-expected-engine", expected);
        }
        let response = router
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&document()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-mixrelay-engine"], "engine-one");
        if expected == Some("old-engine") {
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let error: ProtocolError = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(error.code, ErrorCode::EngineReplaced);
            assert_eq!(error.parameters["operationOutcome"], "not_admitted");
            assert!(backend.last_apply.lock().unwrap().is_none());
        } else {
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            assert!(backend.last_apply.lock().unwrap().is_some());
        }
    }
    for (method, path, status) in [
        (Method::GET, HEALTH_ROUTE, StatusCode::OK),
        (Method::GET, "/missing", StatusCode::NOT_FOUND),
        (Method::POST, HEALTH_ROUTE, StatusCode::METHOD_NOT_ALLOWED),
        (
            Method::GET,
            "/api/v1/operations/unknown",
            StatusCode::NOT_FOUND,
        ),
        (
            Method::DELETE,
            "/api/v1/operations/unknown",
            StatusCode::NOT_FOUND,
        ),
        (Method::PUT, GRAPH_ROUTE, StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-mixrelay-engine"], "engine-one");
        assert_eq!(response.status(), status);
    }
    let response = crate::router(ControlOperations::new(clock()))
        .oneshot(Request::get(HEALTH_ROUTE).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(!response.headers().contains_key("x-mixrelay-engine"));
}

#[tokio::test]
async fn fence_precedes_admission_and_does_not_create_a_receipt() {
    let operations = fast_operations();
    let router = test_router(
        operations.clone(),
        post(|| async { StatusCode::NO_CONTENT }),
    )
    .layer(middleware::from_fn_with_state(
        Some(HeaderValue::from_static("new-engine")),
        engine_identity,
    ));
    operations.close_admission();
    let response = router
        .oneshot(
            Request::post("/work")
                .header("x-mixrelay-expected-engine", "old-engine")
                .header(REQUEST_HEADER, "client-operation-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()[ENGINE_HEADER], "new-engine");
    assert_eq!(operations.lock_ledger().len(), 0);
}
