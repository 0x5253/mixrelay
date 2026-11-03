use super::{
    Arc, Body, ControlOperations, ErrorCode, GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION,
    ProtocolError, Request, ServiceExt, StatusCode, TestBackend, VIRTUAL_ENDPOINT_FORMAT_ROUTE,
    json, router_with_backend,
};

fn accepted() -> Arc<TestBackend> {
    TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 7,
        state: GraphApplyState::Active,
        persistence_error: None,
    }))
}

async fn send(backend: Arc<TestBackend>, body: serde_json::Value) -> axum::response::Response {
    router_with_backend(backend, ControlOperations::new(crate::tests::clock()))
        .oneshot(
            Request::put(VIRTUAL_ENDPOINT_FORMAT_ROUTE)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn endpoint_format_forwards_revision_and_device_rate() {
    let backend = accepted();
    let response = send(
        backend.clone(),
        json!({"expectedRevision":6,"endpointId":"selected-device","sampleRate":96000}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let request = backend.last_endpoint_format.lock().unwrap();
    let request = request.as_ref().unwrap();
    assert_eq!(request.expected_revision, 6);
    assert_eq!(request.sample_rate, 96_000);
    assert_eq!(request.endpoint_id.as_str(), "selected-device");
}

#[tokio::test]
async fn endpoint_format_rejects_malformed_requests_before_backend_mutation() {
    for body in [
        json!({"sampleRate":96000}),
        json!({"expectedRevision":6,"sampleRate":96000}),
        json!({"expectedRevision":6,"sampleRate":-1}),
        json!({"expectedRevision":6,"sampleRate":96000,"graphRate":48000}),
    ] {
        let backend = accepted();
        assert_eq!(
            send(backend.clone(), body).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(backend.last_endpoint_format.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn endpoint_format_preserves_stale_and_unsupported_errors() {
    for (code, status) in [
        (ErrorCode::GraphRevisionStale, StatusCode::CONFLICT),
        (
            ErrorCode::AudioSessionUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let backend = TestBackend::with_result(Err(ProtocolError {
            code,
            parameters: json!({"sampleRate":192_000}),
        }));
        let response = send(
            backend,
            json!({"expectedRevision":6,"endpointId":"selected-device","sampleRate":192_000}),
        )
        .await;
        assert_eq!(response.status(), status);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let error: ProtocolError = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error.code, code);
        assert_eq!(error.parameters, json!({"sampleRate":192_000}));
    }
}
