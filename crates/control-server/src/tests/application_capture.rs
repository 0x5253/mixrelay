use super::*;

fn backend() -> Arc<TestBackend> {
    TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 6,
        state: GraphApplyState::Pending,
        persistence_error: None,
    }))
}

#[tokio::test]
async fn unavailable_manifest_is_retryable_and_not_an_empty_capture_snapshot() {
    let (status, body) = response_json(
        router(ControlOperations::new(crate::tests::clock())),
        Request::get(APPLICATION_CAPTURE_MANIFEST_ROUTE)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "control_unavailable");
    assert!(body.get("sources").is_none());
}

#[tokio::test]
async fn current_and_preview_manifests_do_not_publish_graphs() {
    let backend = backend();
    let (status, body) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::get(APPLICATION_CAPTURE_MANIFEST_ROUTE)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"protocolVersion":PROTOCOL_VERSION,"revision":5,"sources":[]})
    );
    let request: GraphApplyRequest = GraphDocument::new(6, vec![], vec![]).into();
    let (status, body) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::post(APPLICATION_CAPTURE_PREVIEW_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&request).unwrap()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revision"], 6);
    assert_eq!(*backend.last_preview.lock().unwrap(), Some(request));
    assert!(backend.last_apply.lock().unwrap().is_none());
}

#[tokio::test]
async fn malformed_preview_is_rejected_before_backend_and_stale_status_is_preserved() {
    let backend = backend();
    let (status, _) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::post(APPLICATION_CAPTURE_PREVIEW_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(backend.last_preview.lock().unwrap().is_none());
    *backend.apply_result.lock().unwrap() = Err(ProtocolError {
        code: ErrorCode::GraphRevisionStale,
        parameters: json!({"submittedRevision":7}),
    });
    let request: GraphApplyRequest = GraphDocument::new(6, vec![], vec![]).into();
    let (status, body) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::post(APPLICATION_CAPTURE_PREVIEW_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&request).unwrap()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "graph_revision_stale");
    assert!(backend.last_apply.lock().unwrap().is_none());
}
