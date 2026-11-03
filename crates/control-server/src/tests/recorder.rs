use super::*;

fn backend() -> Arc<TestBackend> {
    TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 1,
        state: GraphApplyState::Active,
        persistence_error: None,
    }))
}

#[tokio::test]
async fn recorder_routes_preserve_graph_identity_and_typed_file_options() {
    let backend = backend();
    let path = "/api/v1/graphs/graph%20B/recorders/recorder%201";
    let (status, body) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::get(path).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["path"], "graph B/recorder 1");
    let request = json!({"expectedRevision":5,"request":{"action":"record","path":"/recording.wav","sampleRate":96_000,"encoding":"pcm24"}});
    let (status, _) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::put(path)
            .header("content-type", "application/json")
            .body(Body::from(request.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let recorded = backend.last_recorder.lock().unwrap();
    let (graph, node, actual) = recorded.as_ref().unwrap();
    assert_eq!((graph.as_str(), node.as_str()), ("graph B", "recorder 1"));
    assert_eq!(serde_json::to_value(actual).unwrap(), request);
}

#[tokio::test]
async fn malformed_recorder_controls_never_reach_backend() {
    for body in [
        json!({"expectedRevision":5,"request":{"action":"overwrite"}}),
        json!({"expectedRevision":5,"request":{"action":"record","encoding":"mp3"}}),
        json!({"expectedRevision":5,"request":{"action":"record","sampleRate":-1}}),
        json!({"expectedRevision":5,"request":{"action":"stop","extra":true}}),
        json!({"request":{"action":"stop"}}),
    ] {
        let backend = backend();
        let (status, _) = response_json(
            router_with_backend(
                backend.clone(),
                ControlOperations::new(crate::tests::clock()),
            ),
            Request::put("/api/v1/graphs/a/recorders/b")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(backend.last_recorder.lock().unwrap().is_none());
    }
}
