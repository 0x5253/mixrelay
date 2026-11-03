//! HTTP adapter contracts with deterministic backend observations.
pub(super) fn clock() -> Box<dyn mixrelay_time::MonotonicClock> {
    Box::new(mixrelay_time::ManualClock::new(
        mixrelay_time::Instant::from_origin(std::time::Duration::ZERO),
    ))
}

mod application_capture;
mod endpoint_format;
mod node_control;
mod recorder;
use std::sync::Mutex;

use axum::{
    body::Body,
    http::{Method, Request, header::CONTENT_TYPE},
};
use mixrelay_control_protocol::{GraphApplyState, GraphCatalogResponse, GraphLayoutResponse};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;

mod telemetry;

struct TestBackend {
    incarnation: String,
    last_node_control: Mutex<Option<node_control::RecordedControl>>,
    last_recorder: Mutex<
        Option<(
            String,
            String,
            mixrelay_control_protocol::RecorderControlRequest,
        )>,
    >,
    apply_result: Mutex<Result<GraphApplyResponse, ProtocolError>>,
    last_apply: Mutex<Option<GraphApplyRequest>>,
    last_preview: Mutex<Option<GraphApplyRequest>>,
    last_endpoint_format: Mutex<Option<mixrelay_control_protocol::VirtualEndpointFormatRequest>>,
    last_retry: Mutex<Option<ProcessorRetryRequest>>,
    editor_gate: Option<Arc<EditorGate>>,
}

#[derive(Default)]
struct EditorGate {
    started: std::sync::atomic::AtomicBool,
    released: std::sync::atomic::AtomicBool,
    finished: std::sync::atomic::AtomicBool,
}

impl TestBackend {
    fn with_result(result: Result<GraphApplyResponse, ProtocolError>) -> Arc<Self> {
        Arc::new(Self {
            incarnation: String::new(),
            last_node_control: Mutex::new(None),
            last_recorder: Mutex::new(None),
            apply_result: Mutex::new(result),
            last_apply: Mutex::new(None),
            last_preview: Mutex::new(None),
            last_endpoint_format: Mutex::new(None),
            last_retry: Mutex::new(None),
            editor_gate: None,
        })
    }
}

impl ControlBackend for TestBackend {
    fn telemetry_state(&self) -> Option<mixrelay_control_protocol::ControlStateSnapshot> {
        if self.incarnation == "deferred-observation" {
            let mut admitted = self.last_preview.lock().unwrap();
            if admitted.is_none() {
                *admitted = Some(document().into());
                return None;
            }
        }
        Some(crate::control_backend::snapshot(self))
    }
    fn incarnation(&self) -> String {
        self.incarnation.clone()
    }
    fn controls_snapshot(
        &self,
    ) -> Result<mixrelay_control_protocol::EngineControlSnapshot, ProtocolError> {
        Ok(node_control::snapshot())
    }

    fn node_controls(
        &self,
        graph_id: &str,
    ) -> Result<mixrelay_control_protocol::GraphNodeControls, ProtocolError> {
        Ok(node_control::discovery(graph_id))
    }

    fn preview_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlPreview, ProtocolError> {
        self.update_node_control(graph_id, node_id, request)
            .map(|response| mixrelay_control_protocol::NodeControlPreview {
                capture_manifest: Some(mixrelay_control_protocol::ApplicationCaptureManifest {
                    protocol_version: PROTOCOL_VERSION,
                    revision: response.application.revision,
                    sources: Vec::new(),
                }),
            })
    }

    fn update_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlResponse, ProtocolError> {
        *self.last_node_control.lock().expect("test mutex") = Some(node_control::RecordedControl {
            graph_id: graph_id.to_owned(),
            node_id: node_id.to_owned(),
            request,
        });
        self.apply_graph_request(document().into())
            .map(
                |application| mixrelay_control_protocol::NodeControlResponse {
                    application,
                    requires_graph_reload: false,
                },
            )
    }
    fn recorder_status(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        Ok(mixrelay_audio_io::recording::RecorderStatus {
            path: Some(format!("{graph_id}/{node_id}")),
            ..Default::default()
        })
    }

    fn control_recorder(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::RecorderControlRequest,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        *self.last_recorder.lock().unwrap() =
            Some((graph_id.to_owned(), node_id.to_owned(), request));
        self.recorder_status(graph_id, node_id)
    }
    fn application_capture_manifest(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        Ok(mixrelay_control_protocol::ApplicationCaptureManifest {
            protocol_version: PROTOCOL_VERSION,
            revision: 5,
            sources: vec![],
        })
    }

    fn preview_application_capture(
        &self,
        request: GraphApplyRequest,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        let revision = request.document.revision;
        *self.last_preview.lock().unwrap() = Some(request);
        self.apply_result.lock().unwrap().clone()?;
        Ok(mixrelay_control_protocol::ApplicationCaptureManifest {
            protocol_version: PROTOCOL_VERSION,
            revision,
            sources: vec![],
        })
    }
    fn reconfigure_virtual_endpoint(
        &self,
        request: mixrelay_control_protocol::VirtualEndpointFormatRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        *self.last_endpoint_format.lock().unwrap() = Some(request);
        self.apply_result.lock().unwrap().clone()
    }
    fn graph_layout(&self, id: &str) -> Result<GraphLayoutResponse, ProtocolError> {
        if id != "saved" {
            return Err(ProtocolError {
                code: ErrorCode::GraphInvalid,
                parameters: json!({"resource":"graph_layout"}),
            });
        }
        Ok(GraphLayoutResponse {
            graph_id: id.into(),
            layout_revision: 4,
            layout: mixrelay_control_protocol::GraphLayout::default(),
        })
    }

    fn update_graph_layout(
        &self,
        id: &str,
        request: GraphLayoutRequest,
    ) -> Result<GraphLayoutResponse, ProtocolError> {
        let mut response = self.graph_layout(id)?;
        if request.expected_layout_revision != response.layout_revision {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: json!({"resource":"graph_layout"}),
            });
        }
        response.layout = request.layout;
        response.layout_revision += 1;
        Ok(response)
    }
    fn graph_catalog(&self) -> Result<GraphCatalogResponse, ProtocolError> {
        Ok(GraphCatalogResponse {
            protocol_version: PROTOCOL_VERSION,
            catalog_revision: 7,
            active_id: "saved".into(),
            graphs: vec![mixrelay_control_protocol::GraphCatalogEntry {
                id: "saved".into(),
                name: "Ableton".into(),
            }],
            document: document(),
            layout_revision: 0,
            layout: mixrelay_control_protocol::GraphLayout::default(),
            persistence_error: None,
        })
    }

    fn graph_catalog_document(&self, id: &str) -> Result<GraphDocument, ProtocolError> {
        if id == "saved" {
            return Ok(document());
        }
        Err(ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: json!({}),
        })
    }

    fn mutate_graph_catalog(
        &self,
        request: GraphCatalogRequest,
    ) -> Result<GraphCatalogResponse, ProtocolError> {
        if request.expected_revision != 5 || request.expected_catalog_revision != 7 {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: json!({}),
            });
        }
        let mixrelay_control_protocol::GraphCatalogAction::Rename { id, name } = request.action
        else {
            return Err(ProtocolError {
                code: ErrorCode::GraphInvalid,
                parameters: json!({}),
            });
        };
        let mut response = self.graph_catalog()?;
        assert_eq!(id, "saved");
        response.graphs[0].name = name;
        response.catalog_revision += 1;
        Ok(response)
    }
    fn status(&self) -> AppStatus {
        AppStatus {
            driver_state: mixrelay_control_protocol::DriverState::Ready,
            sample_rate: Some(48_000),
            buffer_frames: Some(128),
            processor_latencies: vec![mixrelay_control_protocol::ProcessorLatency {
                graph_id: "graph-a".to_owned(),
                node_id: "effect".to_owned(),
                transport_frames: 128,
                intrinsic_frames: None,
            }],
            failed_processors: vec![mixrelay_control_protocol::FailedProcessor {
                graph_id: "graph-a".to_owned(),
                node_id: "effect".to_owned(),
                instance_id: "18446744073709551615".to_owned(),
            }],
            ..AppStatus::default()
        }
    }

    fn graph_status(&self) -> GraphRuntimeStatus {
        GraphRuntimeStatus {
            pending_reason: None,
            protocol_version: PROTOCOL_VERSION,
            active_revision: 4,
            submitted_revision: 5,
        }
    }

    fn graph(&self) -> GraphDocument {
        assert_ne!(self.incarnation, "panic-observation");
        let mut graph = document();
        if self.incarnation == "editor-observation" {
            graph.nodes = serde_json::from_value(serde_json::json!([{
                "id": "effect", "type": "gain", "config": {"channels": 2, "gainDb": 0.0}
            }]))
            .unwrap();
        }
        graph
    }

    fn meters(&self) -> MeterSnapshot {
        MeterSnapshot {
            protocol_version: PROTOCOL_VERSION,
            revision: 4,
            nodes: Vec::new(),
        }
    }

    fn processors(&self) -> ExternalProcessorCatalog {
        ExternalProcessorCatalog {
            protocol_version: PROTOCOL_VERSION,
            processors: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn rescan_processors(
        &self,
        _request: ProcessorScanRequest,
    ) -> Result<ExternalProcessorCatalog, ProtocolError> {
        Ok(self.processors())
    }

    fn set_processor_parameter(
        &self,
        node_id: &str,
        parameter_id: u32,
        update: ProcessorParameterUpdate,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError> {
        Ok(ProcessorParameterUpdateResponse {
            protocol_version: PROTOCOL_VERSION,
            node_id: node_id.to_owned(),
            parameter_id,
            value: update.value,
            persistence_error: (node_id == "unsaved-node")
                .then(|| "fixture disk unavailable".to_owned()),
        })
    }

    fn processor_editor_status(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        Ok(test_editor_status(node_id, false))
    }

    fn set_processor_editor(
        &self,
        node_id: &str,
        request: ProcessorEditorRequest,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        if let Some(gate) = &self.editor_gate {
            use std::{
                sync::atomic::Ordering,
                time::{Duration, Instant},
            };
            gate.started.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(2);
            while !gate.released.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::park_timeout(Duration::from_millis(1));
            }
            gate.finished.store(true, Ordering::Release);
        }
        Ok(test_editor_status(node_id, request.open))
    }

    fn retry_failed_processor(
        &self,
        request: ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        *self.last_retry.lock().unwrap() = Some(request);
        self.apply_result.lock().unwrap().clone()
    }

    fn apply_graph_request(
        &self,
        request: GraphApplyRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        *self.last_apply.lock().unwrap() = Some(request);
        self.apply_result
            .lock()
            .expect("test result should not be poisoned")
            .clone()
    }
}

#[tokio::test]
async fn engine_header_and_fence_cover_success_errors_and_fallbacks() {
    let mut backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        persistence_error: None,
        revision: 2,
        state: mixrelay_control_protocol::GraphApplyState::Pending,
    }));
    Arc::get_mut(&mut backend).unwrap().incarnation = "engine-one".into();
    let operations = ControlOperations::new(crate::tests::clock());
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
    let response = super::router(ControlOperations::new(crate::tests::clock()))
        .oneshot(Request::get(HEALTH_ROUTE).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(!response.headers().contains_key("x-mixrelay-engine"));
}

fn document() -> GraphDocument {
    GraphDocument::new(5, vec![], vec![])
}

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
    let router = router_with_backend(
        backend.clone(),
        ControlOperations::new(crate::tests::clock()),
    );
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

#[tokio::test(flavor = "current_thread")]
async fn native_editor_checkpoint_does_not_block_status_on_the_io_executor() {
    use std::{sync::atomic::Ordering, time::Duration};
    let gate = Arc::new(EditorGate::default());
    let backend = Arc::new(TestBackend {
        incarnation: String::new(),
        last_node_control: Mutex::new(None),
        last_recorder: Mutex::new(None),
        apply_result: Mutex::new(Err(processor_unavailable("unused"))),
        last_apply: Mutex::new(None),
        last_preview: Mutex::new(None),
        last_endpoint_format: Mutex::new(None),
        last_retry: Mutex::new(None),
        editor_gate: Some(gate.clone()),
    });
    let router = router_with_backend(backend, ControlOperations::new(crate::tests::clock()));
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

fn test_editor_status(node_id: &str, open: bool) -> ProcessorEditorStatus {
    ProcessorEditorStatus {
        protocol_version: PROTOCOL_VERSION,
        node_id: node_id.to_owned(),
        state: if open {
            mixrelay_control_protocol::ProcessorEditorState::Open
        } else {
            mixrelay_control_protocol::ProcessorEditorState::Closed
        },
        width: open.then_some(640),
        height: open.then_some(480),
        resizable: open,
        failure: None,
    }
}

async fn response_json(router: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.oneshot(request).await.expect("request should run");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("response should have a body");
    let value = serde_json::from_slice(&bytes).expect("response should be JSON");
    (status, value)
}

#[tokio::test]
async fn meter_telemetry_streams_every_window_until_admission_closes() {
    use futures_util::StreamExt as _;
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 4,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    let operations = ControlOperations::new(crate::tests::clock());
    let response = router_with_backend(backend, operations.clone())
        .oneshot(
            Request::builder()
                .uri(METER_TELEMETRY_ROUTE)
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
async fn graph_layout_routes_preserve_ui_identities_and_use_independent_revision() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    let route = "/api/v1/graphs/saved/layout";
    let (status, body) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::builder().uri(route).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["graphId"], "saved");
    assert_eq!(body["layoutRevision"], 4);
    let layout = json!({"positions":{"duplex:input":{"x":-50.0,"y":75.0}},"viewport":{"x":-100.0,"y":80.0,"zoom":0.5}});
    for (revision, expected_status) in [(4, StatusCode::OK), (3, StatusCode::CONFLICT)] {
        let (status, body) = response_json(
            router_with_backend(
                backend.clone(),
                ControlOperations::new(crate::tests::clock()),
            ),
            Request::builder()
                .method(Method::PUT)
                .uri(route)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"expectedLayoutRevision":revision,"layout":layout}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, expected_status);
        if status == StatusCode::OK {
            assert_eq!(body["layout"], layout);
            assert_eq!(body["layoutRevision"], 5);
        } else {
            assert_eq!(body["code"], "graph_revision_stale");
            assert_eq!(body["parameters"]["resource"], "graph_layout");
        }
    }
    let invalid =
        json!({"expectedLayoutRevision":4,"layout":{"positions":{"node":{"x":null,"y":1}}}});
    let (status, body) = response_json(
        router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
        Request::builder()
            .method(Method::PUT)
            .uri(route)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(invalid.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["parameters"]["resource"], "graph_layout");
    assert_eq!(body["parameters"]["graphId"], "saved");
}

#[tokio::test]
async fn catalog_routes_preserve_document_identity_and_both_concurrency_baselines() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    for route in [GRAPH_CATALOG_ROUTE, "/api/v1/graphs/saved"] {
        let (status, body) = response_json(
            router_with_backend(
                backend.clone(),
                ControlOperations::new(crate::tests::clock()),
            ),
            Request::builder().uri(route).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if route == GRAPH_CATALOG_ROUTE {
            assert_eq!(body["activeId"], "saved");
            assert_eq!(body["catalogRevision"], 7);
            assert_eq!(body["document"]["revision"], 5);
        } else {
            assert_eq!(body["revision"], 5);
        }
    }
    for (revision, expected_status) in [(7, StatusCode::OK), (6, StatusCode::CONFLICT)] {
        let (status, body) = response_json(router_with_backend(backend.clone(), ControlOperations::new(crate::tests::clock())), Request::builder().method(Method::PUT).uri(GRAPH_CATALOG_ROUTE).header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"action":"rename","id":"saved","name":"SPAT","expectedRevision":5,"expectedCatalogRevision":revision}).to_string())).unwrap()).await;
        assert_eq!(status, expected_status);
        if status == StatusCode::OK {
            assert_eq!(body["graphs"][0]["name"], "SPAT");
        } else {
            assert_eq!(body["code"], "graph_revision_stale");
            assert_eq!(body["parameters"]["catalogOutcome"], "not_applied");
        }
    }
    let (status, _) = response_json(
        router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
        Request::builder()
            .method(Method::PUT)
            .uri(GRAPH_CATALOG_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn default_routes_are_loopback_safe_and_versioned() {
    let health = router(ControlOperations::new(crate::tests::clock()))
        .oneshot(
            Request::builder()
                .uri(HEALTH_ROUTE)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("health request should run");
    let health_body = axum::body::to_bytes(health.into_body(), 16)
        .await
        .expect("health should have a body");
    assert_eq!(&health_body[..], b"ok");

    let (_, status) = response_json(
        router(ControlOperations::new(crate::tests::clock())),
        Request::builder()
            .uri(STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(status["driverState"], "not_installed");

    let (_, graph) = response_json(
        router(ControlOperations::new(crate::tests::clock())),
        Request::builder()
            .uri(GRAPH_STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph["activeRevision"], 0);

    let (_, graph_document) = response_json(
        router(ControlOperations::new(crate::tests::clock())),
        Request::builder()
            .uri(GRAPH_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph_document["revision"], 0);

    let (status, error) = apply_request(
        router(ControlOperations::new(crate::tests::clock())),
        &document(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "control_unavailable");
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
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
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
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::builder()
            .uri(GRAPH_STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph["activeRevision"], 4);
    assert_eq!(graph["submittedRevision"], 5);

    let (_, graph_document) = response_json(
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::builder()
            .uri(GRAPH_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph_document["revision"], 5);

    let (status, response) = apply_request(
        router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
        &document(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response["revision"], 5);
    assert_eq!(response["state"], "pending");
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
        router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
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
        router_with_backend(backend.clone(), ControlOperations::new(crate::tests::clock())),
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
        router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
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
        router_with_backend(
            backend.clone(),
            ControlOperations::new(crate::tests::clock()),
        ),
        Request::builder()
            .uri("/api/v1/processors/processor-node/editor")
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["state"], "closed");

    let (status, opened) = response_json(
        router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
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

#[tokio::test]
async fn typed_graph_failures_map_to_stable_http_statuses() {
    let cases = [
        (ErrorCode::EngineReplaced, StatusCode::CONFLICT),
        (ErrorCode::InvalidOperationId, StatusCode::BAD_REQUEST),
        (ErrorCode::GraphRevisionStale, StatusCode::CONFLICT),
        (ErrorCode::GraphExchangeBusy, StatusCode::CONFLICT),
        (ErrorCode::GraphInvalid, StatusCode::UNPROCESSABLE_ENTITY),
        (
            ErrorCode::GraphBindingUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::Unauthorized, StatusCode::UNAUTHORIZED),
        (ErrorCode::PairingRequired, StatusCode::UNAUTHORIZED),
        (ErrorCode::DriverNotLoaded, StatusCode::SERVICE_UNAVAILABLE),
    ];
    for (code, expected_status) in cases {
        let backend = TestBackend::with_result(Err(ProtocolError {
            code,
            parameters: json!({ "revision": 5 }),
        }));
        let (status, response) = apply_request(
            router_with_backend(backend, ControlOperations::new(crate::tests::clock())),
            &document(),
        )
        .await;
        assert_eq!(status, expected_status);
        assert_eq!(response["parameters"]["revision"], 5);
    }
}

async fn apply_request(router: Router, document: &GraphDocument) -> (StatusCode, Value) {
    response_json(
        router,
        Request::builder()
            .method("PUT")
            .uri(GRAPH_ROUTE)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(document).expect("document should serialize"),
            ))
            .expect("request should build"),
    )
    .await
}
