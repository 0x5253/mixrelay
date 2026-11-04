use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::CONTENT_TYPE},
};
use mixrelay_control_protocol::{
    ErrorCode, GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION, ProcessorRetryRequest,
    ProtocolError,
};
use serde_json::json;
use tower::ServiceExt;

use crate::{
    ControlOperations, PROCESSOR_RETRY_ROUTE, STATUS_ROUTE, router_with_backend,
    test_support::{EditorGate, TestBackend, clock, response_json},
};

#[tokio::test(flavor = "current_thread")]
async fn native_editor_checkpoint_does_not_block_status_on_the_io_executor() {
    use std::{sync::atomic::Ordering, time::Duration};
    let gate = Arc::new(EditorGate::default());
    let mut backend = TestBackend::with_result(Err(ProtocolError {
        code: ErrorCode::ProcessorUnavailable,
        parameters: json!({"nodeId": "unused"}),
    }));
    Arc::get_mut(&mut backend).unwrap().editor_gate = Some(gate.clone());
    let router = router_with_backend(backend, ControlOperations::new(clock()));
    let close_router = router.clone();
    let close = tokio::spawn(async move {
        response_json(
            close_router,
            Request::builder()
                .method("PUT")
                .uri("/api/v1/processors/effect/editor")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"expectedRevision":5,"open":false}"#))
                .unwrap(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !gate.started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let status = tokio::time::timeout(
        Duration::from_millis(100),
        router.oneshot(
            Request::builder()
                .uri(STATUS_ROUTE)
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let was_waiting = !gate.finished.load(Ordering::Acquire);
    gate.released.store(true, Ordering::Release);
    assert_eq!(status.status(), StatusCode::OK);
    assert!(
        was_waiting,
        "status must complete before the native checkpoint is released"
    );
    assert_eq!(close.await.unwrap().0, StatusCode::OK);
}

#[tokio::test]
async fn processor_parameters_use_a_typed_node_route() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Pending,
        persistence_error: None,
    }));
    let (status, response) = response_json(
        router_with_backend(backend, ControlOperations::new(clock())),
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/processors/vst-node/parameters/42")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"expectedRevision":5,"value":0.75}"#))
            .expect("request should build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["nodeId"], "vst-node");
    assert_eq!(response["parameterId"], 42);
    assert_eq!(response["value"], 0.75);
}

#[tokio::test]
async fn failed_processor_retry_uses_a_typed_revision_and_instance_guarded_route() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 6,
        state: GraphApplyState::Pending,
        persistence_error: None,
    }));
    let (status, response) = response_json(
        router_with_backend(backend.clone(), ControlOperations::new(clock())),
        Request::builder()
            .method(Method::PUT)
            .uri(PROCESSOR_RETRY_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"graphId":"graph-a","nodeId":"effect","expectedRevision":5,"expectedCatalogRevision":7,"expectedInstanceId":"18446744073709551615"}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["revision"], 6);
    assert_eq!(
        *backend.last_retry.lock().unwrap(),
        Some(ProcessorRetryRequest {
            graph_id: "graph-a".into(),
            node_id: "effect".into(),
            expected_revision: 5,
            expected_catalog_revision: 7,
            expected_instance_id: "18446744073709551615".into(),
        })
    );
}

#[tokio::test]
async fn accepted_processor_parameter_storage_warning_remains_http_success() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    let (status, response) = response_json(
        router_with_backend(backend, ControlOperations::new(clock())),
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/processors/unsaved-node/parameters/42")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"expectedRevision":5,"value":0.75}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["value"], 0.75);
    assert_eq!(response["persistenceError"], "fixture disk unavailable");
}

#[tokio::test]
async fn processor_editor_uses_typed_get_and_idempotent_put_routes() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Pending,
        persistence_error: None,
    }));
    let (status, closed) = response_json(
        router_with_backend(backend.clone(), ControlOperations::new(clock())),
        Request::builder()
            .uri("/api/v1/processors/processor-node/editor")
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["state"], "closed");

    let (status, opened) = response_json(
        router_with_backend(backend, ControlOperations::new(clock())),
        Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/processors/processor-node/editor")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"expectedRevision":5,"open":true}"#))
            .expect("request should build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opened["nodeId"], "processor-node");
    assert_eq!(opened["state"], "open");
    assert_eq!(opened["width"], 640);
    assert_eq!(opened["height"], 480);
    assert_eq!(opened["resizable"], true);
}
