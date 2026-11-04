use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mixrelay_control_protocol::{
    ErrorCode, GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION, ProtocolError,
};
use serde_json::{Value, json};

use crate::{
    CONTROL_SNAPSHOT_ROUTE, ControlOperations, router_with_backend,
    test_support::{TestBackend, clock, response_json},
};
use mixrelay_control_protocol::NodeControlRequest;

use crate::test_support::{RecordedControl, control_snapshot};

#[tokio::test]
async fn snapshot_returns_the_engine_observation_even_while_a_revision_is_pending() {
    let (status, body) = response_json(
        router_with_backend(backend(), ControlOperations::new(clock())),
        Request::get(CONTROL_SNAPSHOT_ROUTE)
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::to_value(control_snapshot()).expect("snapshot")
    );
    assert_eq!(body["graphs"][0]["nodes"][0]["nodeId"], "effect");
}

fn backend() -> Arc<TestBackend> {
    TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 8,
        state: GraphApplyState::Pending,
        persistence_error: None,
    }))
}

fn request(method: &str, suffix: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!(
            "/api/v1/graphs/graph%20B/nodes/effect%201/control{suffix}"
        ))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

#[tokio::test]
async fn discovery_is_graph_scoped_and_does_not_invent_recorder_actions() {
    let (status, body) = response_json(
        router_with_backend(backend(), ControlOperations::new(clock())),
        Request::get("/api/v1/graphs/graph%20B/nodes/controls")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["graphId"], "graph B");
    assert_eq!(body["nodes"][0]["capabilities"]["recorder"], false);
}

#[tokio::test]
async fn update_and_preview_forward_identical_explicit_targets() {
    for (method, suffix, expected_status) in [
        ("PUT", "", StatusCode::ACCEPTED),
        ("POST", "/preview", StatusCode::OK),
    ] {
        let backend = backend();
        let (status, body) = response_json(
            router_with_backend(backend.clone(), ControlOperations::new(clock())),
            request(
                method,
                suffix,
                &json!({"expectedRevision":7,"enabled":false,"muted":true}),
            ),
        )
        .await;
        assert_eq!(status, expected_status);
        if method == "POST" {
            assert_eq!(body["captureManifest"]["revision"], 8);
        } else {
            assert_eq!(body["revision"], 8);
            assert_eq!(body["requiresGraphReload"], false);
        }
        assert_eq!(
            *backend.last_node_control.lock().expect("test mutex"),
            Some(RecordedControl {
                graph_id: "graph B".into(),
                node_id: "effect 1".into(),
                request: NodeControlRequest {
                    expected_revision: 7,
                    enabled: Some(false),
                    muted: Some(true),
                    gain_db: None,
                },
            })
        );
    }
}

#[tokio::test]
async fn stale_missing_disabled_and_failed_controls_are_definitive_typed_errors() {
    for (code, expected_status) in [
        (ErrorCode::GraphRevisionStale, StatusCode::CONFLICT),
        (ErrorCode::NodeNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::NodeDisabled, StatusCode::CONFLICT),
        (
            ErrorCode::ControlUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let backend = TestBackend::with_result(Err(ProtocolError {
            code,
            parameters: json!({"nodeId":"effect 1"}),
        }));
        let (status, body) = response_json(
            router_with_backend(backend, ControlOperations::new(clock())),
            request("PUT", "", &json!({"expectedRevision":7,"enabled":true})),
        )
        .await;
        assert_eq!(status, expected_status);
        assert_eq!(
            body["code"],
            serde_json::to_value(code).expect("error code")
        );
        assert_eq!(body["parameters"]["nodeControlOutcome"], "not_applied");
    }
}

#[tokio::test]
async fn malformed_controls_never_reach_backend() {
    for body in [
        json!({"enabled":false}),
        json!({"expectedRevision":7,"toggle":true}),
        json!({"expectedRevision":7,"muted":"yes"}),
    ] {
        let backend = backend();
        let (status, body) = response_json(
            router_with_backend(backend.clone(), ControlOperations::new(clock())),
            request("PUT", "", &body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "graph_invalid");
        assert!(
            backend
                .last_node_control
                .lock()
                .expect("test mutex")
                .is_none()
        );
    }
}

#[tokio::test]
async fn parameter_and_editor_writes_without_revision_are_rejected() {
    for (path, body) in [
        (
            "/api/v1/processors/effect/parameters/1",
            json!({"value":0.5}),
        ),
        ("/api/v1/processors/effect/editor", json!({"open":true})),
    ] {
        let (status, body) = response_json(
            router_with_backend(backend(), ControlOperations::new(clock())),
            Request::put(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "graph_invalid");
    }
}
