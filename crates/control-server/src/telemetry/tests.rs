use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use mixrelay_control_protocol::{
    GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION, TelemetryDecoder,
};
use tower::ServiceExt;

use super::*;
use crate::{
    CONTROL_TELEMETRY_ROUTE, router_with_backend,
    test_support::{TestBackend, clock},
};

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

#[tokio::test(start_paused = true)]
async fn reconnect_and_late_subscribers_get_full_state_from_the_shared_publisher() {
    let operations = ControlOperations::new(clock());
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
    let operations = ControlOperations::new(clock());
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
    let operations = ControlOperations::new(clock());
    let router = router_with_backend(backend, operations.clone());
    let mut stream = subscribe(&router).await;
    assert!(stream.ended().await);
    operations.close_admission();
}

#[tokio::test(start_paused = true)]
async fn deferred_observations_wait_for_initial_snapshot_and_keep_committed_state() {
    let mut backend = backend();
    Arc::get_mut(&mut backend).unwrap().incarnation = "deferred-observation".into();
    let operations = ControlOperations::new(clock());
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

#[tokio::test(start_paused = true)]
async fn meter_telemetry_streams_every_window_until_admission_closes() {
    use futures_util::StreamExt as _;
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 4,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    let operations = ControlOperations::new(clock());
    let response = router_with_backend(backend, operations.clone())
        .oneshot(
            Request::builder()
                .uri(CONTROL_TELEMETRY_ROUTE)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should run");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    let mut events = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .expect("first window")
        .expect("open stream")
        .expect("event bytes");
    let text = String::from_utf8(first.to_vec()).expect("utf-8 event");
    assert!(text.starts_with("event: telemetry\n"), "{text}");
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("event data");
    let telemetry: mixrelay_control_protocol::ControlTelemetry =
        serde_json::from_str(data).expect("telemetry JSON");
    assert_eq!(telemetry.sequence, 1);
    assert_eq!(telemetry.meters.revision, 4);
    // The test backend has no engine identity.
    assert_eq!(telemetry.engine_incarnation, "");
    // A steady window is sent again, so subscribers can tell it from a stall.
    let second = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
        .await
        .expect("second window")
        .expect("open stream")
        .expect("event bytes");
    assert!(String::from_utf8_lossy(&second).contains("\"sequence\":2"));
    operations.close_admission();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while events.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "closing admission ends the stream");
}

#[tokio::test]
async fn closing_admission_ends_the_shared_publisher_itself() {
    let publisher = Publisher::default();
    let operations = ControlOperations::new(clock());
    operations.close_admission();
    let mut subscription = publisher.subscribe(Arc::new(crate::UnavailableBackend), operations);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), subscription.next())
            .await
            .unwrap()
            .is_none()
    );
}

fn window(sequence: u64, peak: f32) -> ControlTelemetry {
    ControlTelemetry {
        engine_incarnation: "engine".into(),
        sequence,
        meters: mixrelay_control_protocol::MeterSnapshot {
            protocol_version: 1,
            revision: 1,
            nodes: vec![mixrelay_control_protocol::NodeMeterSnapshot {
                node_id: "n".into(),
                port_id: "out".into(),
                peaks: vec![peak],
                dynamics: None,
            }],
        },
        state: None,
    }
}

#[tokio::test]
async fn a_slow_subscriber_folds_backlogged_windows_and_ends_when_too_far_behind() {
    let (sender, latest) = watch::channel(Some(window(5, 0.2)));
    let (windows, receiver) = broadcast::channel(BACKLOG);
    let mut subscription = Subscription {
        latest,
        windows: receiver,
        started: false,
        sent: 0,
    };
    for (sequence, peak) in [(4, 1.0), (6, 0.9), (7, 0.1)] {
        windows.send(window(sequence, peak)).unwrap();
    }
    // The snapshot first, folded with what piled up; older windows are skipped.
    let first = subscription.next().await.unwrap();
    assert_eq!((first.sequence, first.meters.nodes[0].peaks[0]), (7, 0.9));
    windows.send(window(7, 1.0)).unwrap();
    windows.send(window(8, 0.3)).unwrap();
    assert_eq!(subscription.next().await.unwrap().sequence, 8);
    for sequence in 9..=9 + BACKLOG as u64 {
        windows.send(window(sequence, 0.0)).unwrap();
    }
    assert!(subscription.next().await.is_none());
    drop(sender);
    // A snapshot whose windows were overwritten before the first read also ends.
    let (_sender, latest) = watch::channel(Some(window(1, 0.0)));
    let (windows, receiver) = broadcast::channel(BACKLOG);
    for sequence in 2..=2 + BACKLOG as u64 {
        windows.send(window(sequence, 0.0)).unwrap();
    }
    let mut lagged = Subscription {
        latest,
        windows: receiver,
        started: false,
        sent: 0,
    };
    assert!(lagged.next().await.is_none());
    // A publisher that ended still delivers its last window, then the stream ends.
    let (_sender, latest) = watch::channel(None);
    let (windows, receiver) = broadcast::channel(BACKLOG);
    windows.send(window(1, 0.5)).unwrap();
    drop(windows);
    let mut ended = Subscription {
        latest,
        windows: receiver,
        started: false,
        sent: 0,
    };
    assert_eq!(ended.next().await.unwrap().sequence, 1);
    assert!(ended.next().await.is_none());
}
