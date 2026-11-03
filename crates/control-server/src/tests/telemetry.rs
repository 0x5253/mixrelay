use super::*;
use futures_util::StreamExt as _;
use mixrelay_control_protocol::{ControlTelemetry, TelemetryDecoder};

fn backend() -> Arc<TestBackend> {
    TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: 1,
        revision: 4,
        state: GraphApplyState::Active,
        persistence_error: None,
    }))
}

type EventBody = std::pin::Pin<
    Box<dyn futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Send>,
>;

/// One subscription's events, decoded as an ordered stream (bindings may be omitted).
struct Events {
    body: EventBody,
    decoder: TelemetryDecoder,
}

impl Events {
    async fn event(&mut self) -> ControlTelemetry {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), self.body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        self.decoder
            .json(serde_json::from_str(data).unwrap())
            .unwrap()
    }

    async fn ended(&mut self) -> bool {
        tokio::time::timeout(std::time::Duration::from_secs(2), self.body.next())
            .await
            .unwrap()
            .is_none()
    }
}

async fn subscribe(router: &Router) -> Events {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(CONTROL_TELEMETRY_ROUTE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    Events {
        body: Box::pin(response.into_body().into_data_stream()),
        decoder: TelemetryDecoder::default(),
    }
}

#[tokio::test]
async fn reconnect_and_late_subscribers_get_full_state_from_the_shared_publisher() {
    let operations = ControlOperations::new(crate::tests::clock());
    let router = router_with_backend(backend(), operations.clone());
    let mut first = subscribe(&router).await;
    let initial = first.event().await;
    let state = initial.state.unwrap();
    assert_eq!(state.generation, 1);
    assert_eq!(state.graph.active_revision, 4);
    assert!(state.controls.is_some());
    assert!(state.editors.is_empty());
    assert!(first.event().await.state.is_none());
    let mut second = subscribe(&router).await;
    let replay = second.event().await;
    assert_eq!(replay.state.unwrap(), state);
    assert!(replay.sequence >= 2);
    drop(first);
    operations.close_admission();
    assert!(second.ended().await);
}

#[tokio::test]
async fn native_editor_state_is_part_of_the_initial_typed_snapshot() {
    let mut backend = backend();
    Arc::get_mut(&mut backend).unwrap().incarnation = "editor-observation".into();
    let operations = ControlOperations::new(crate::tests::clock());
    let router = router_with_backend(backend, operations.clone());
    let mut stream = subscribe(&router).await;
    let initial = stream.event().await;
    let editors = initial.state.unwrap().editors;
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0].node_id, "effect");
    assert_eq!(
        editors[0].state,
        mixrelay_control_protocol::ProcessorEditorState::Closed
    );
    operations.close_admission();
}

#[tokio::test]
async fn failed_native_observation_ends_subscriptions_instead_of_freezing_state() {
    let mut backend = backend();
    Arc::get_mut(&mut backend).unwrap().incarnation = "panic-observation".into();
    let operations = ControlOperations::new(crate::tests::clock());
    let router = router_with_backend(backend, operations.clone());
    let mut stream = subscribe(&router).await;
    assert!(stream.ended().await);
    operations.close_admission();
}

#[tokio::test]
async fn deferred_observations_wait_for_initial_snapshot_and_keep_committed_state() {
    let mut backend = backend();
    Arc::get_mut(&mut backend).unwrap().incarnation = "deferred-observation".into();
    let operations = ControlOperations::new(crate::tests::clock());
    let router = router_with_backend(backend.clone(), operations.clone());
    let mut stream = subscribe(&router).await;
    let initial = stream.event().await;
    assert!(initial.sequence >= 2);
    let state = initial.state.unwrap();
    *backend.last_preview.lock().unwrap() = None;
    let deferred = stream.event().await;
    assert!(deferred.sequence > initial.sequence);
    assert!(deferred.state.is_none());
    let mut replay = subscribe(&router).await;
    assert_eq!(replay.event().await.state.unwrap(), state);
    operations.close_admission();
}
