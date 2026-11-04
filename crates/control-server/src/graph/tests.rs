use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mixrelay_control_protocol::{
    ErrorCode, GraphApplyRequest, GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION,
};
use serde_json::Value;

use crate::{
    ControlBackend as _, ControlOperations, GRAPH_ROUTE, GRAPH_STATUS_ROUTE, STATUS_ROUTE,
    UnavailableBackend, router_with_backend,
    test_support::{TestBackend, apply_request, clock, document, response_json},
};

#[tokio::test]
async fn graph_apply_forwards_explicit_parameter_intent_and_rejects_malformed_entries() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    let mut request = GraphApplyRequest::from(document());
    request.processor_parameter_changes.push(
        mixrelay_control_protocol::GraphProcessorParameterChange {
            node_id: "effect".into(),
            type_id: "vst3:fixture".into(),
            parameter_id: 7,
            value: 0.25,
        },
    );
    let router = router_with_backend(backend.clone(), ControlOperations::new(clock()));
    let (status, _) = response_json(
        router.clone(),
        Request::builder()
            .method("PUT")
            .uri(GRAPH_ROUTE)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&request).unwrap()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(*backend.last_apply.lock().unwrap(), Some(request.clone()));
    let mut invalid = serde_json::to_value(&request).unwrap();
    invalid["processorParameterChanges"][0]["value"] = Value::Null;
    let (status, body) = response_json(
        router,
        Request::builder()
            .method("PUT")
            .uri(GRAPH_ROUTE)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&invalid).unwrap()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "graph_invalid");
    assert_eq!(*backend.last_apply.lock().unwrap(), Some(request.clone()));
    assert_eq!(
        UnavailableBackend
            .apply_graph_request(request)
            .unwrap_err()
            .code,
        ErrorCode::ControlUnavailable,
        "an unavailable backend rejects the complete request"
    );
}

#[tokio::test]
async fn backend_status_and_graph_application_are_exposed() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Pending,
        persistence_error: None,
    }));
    let (_, status) = response_json(
        router_with_backend(backend.clone(), ControlOperations::new(clock())),
        Request::builder()
            .uri(STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(status["driverState"], "ready");
    assert_eq!(status["processorLatencies"][0]["graphId"], "graph-a");
    assert_eq!(status["processorLatencies"][0]["nodeId"], "effect");
    assert_eq!(status["processorLatencies"][0]["transportFrames"], 128);
    assert!(status["processorLatencies"][0]["intrinsicFrames"].is_null());
    assert_eq!(
        status["failedProcessors"][0]["instanceId"],
        "18446744073709551615"
    );
    assert!(status.get("processorTimings").is_none());

    let (_, graph) = response_json(
        router_with_backend(backend.clone(), ControlOperations::new(clock())),
        Request::builder()
            .uri(GRAPH_STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph["activeRevision"], 4);
    assert_eq!(graph["submittedRevision"], 5);

    let (_, graph_document) = response_json(
        router_with_backend(backend.clone(), ControlOperations::new(clock())),
        Request::builder()
            .uri(GRAPH_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph_document["revision"], 5);

    let (status, response) = apply_request(
        router_with_backend(backend, ControlOperations::new(clock())),
        &document(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response["revision"], 5);
    assert_eq!(response["state"], "pending");
}
