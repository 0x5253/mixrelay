//! A deterministic control backend and request helpers shared by the HTTP tests.
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use mixrelay_audio_graph::{GraphDocument, NodeControl};
use mixrelay_control_protocol::{
    AppStatus, ErrorCode, ExternalProcessorCatalog, GraphApplyRequest, GraphApplyResponse,
    GraphCatalogRequest, GraphCatalogResponse, GraphLayoutRequest, GraphLayoutResponse,
    GraphNodeControls, GraphRuntimeStatus, MeterSnapshot, NodeControlCapabilities,
    NodeControlDescriptor, NodeControlRequest, PROTOCOL_VERSION, ProcessorEditorRequest,
    ProcessorEditorStatus, ProcessorParameterUpdate, ProcessorParameterUpdateResponse,
    ProcessorRetryRequest, ProcessorScanRequest, ProtocolError,
};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{ControlBackend, GRAPH_ROUTE};

pub(crate) fn clock() -> Box<dyn mixrelay_time::MonotonicClock> {
    Box::new(mixrelay_time::ManualClock::new(
        mixrelay_time::Instant::from_origin(std::time::Duration::ZERO),
    ))
}

pub(crate) struct TestBackend {
    pub(crate) incarnation: String,
    pub(crate) last_node_control: Mutex<Option<RecordedControl>>,
    pub(crate) last_recorder: Mutex<
        Option<(
            String,
            String,
            mixrelay_control_protocol::RecorderControlRequest,
        )>,
    >,
    pub(crate) apply_result: Mutex<Result<GraphApplyResponse, ProtocolError>>,
    pub(crate) last_apply: Mutex<Option<GraphApplyRequest>>,
    pub(crate) last_preview: Mutex<Option<GraphApplyRequest>>,
    pub(crate) last_endpoint_format:
        Mutex<Option<mixrelay_control_protocol::VirtualEndpointFormatRequest>>,
    pub(crate) last_retry: Mutex<Option<ProcessorRetryRequest>>,
    pub(crate) editor_gate: Option<Arc<EditorGate>>,
}

#[derive(Default)]
pub(crate) struct EditorGate {
    pub(crate) started: std::sync::atomic::AtomicBool,
    pub(crate) released: std::sync::atomic::AtomicBool,
    pub(crate) finished: std::sync::atomic::AtomicBool,
}

impl TestBackend {
    pub(crate) fn with_result(result: Result<GraphApplyResponse, ProtocolError>) -> Arc<Self> {
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
        Ok(control_snapshot())
    }

    fn node_controls(
        &self,
        graph_id: &str,
    ) -> Result<mixrelay_control_protocol::GraphNodeControls, ProtocolError> {
        Ok(node_controls(graph_id))
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
        *self.last_node_control.lock().expect("test mutex") = Some(RecordedControl {
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
        Ok(editor_status(node_id, false))
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
        Ok(editor_status(node_id, request.open))
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

pub(crate) fn document() -> GraphDocument {
    GraphDocument::new(5, vec![], vec![])
}

pub(crate) fn editor_status(node_id: &str, open: bool) -> ProcessorEditorStatus {
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

pub(crate) async fn response_json(router: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.oneshot(request).await.expect("request should run");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("response should have a body");
    let value = serde_json::from_slice(&bytes).expect("response should be JSON");
    (status, value)
}

pub(crate) async fn apply_request(router: Router, document: &GraphDocument) -> (StatusCode, Value) {
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

#[derive(Debug, PartialEq)]
pub(crate) struct RecordedControl {
    pub(crate) graph_id: String,
    pub(crate) node_id: String,
    pub(crate) request: NodeControlRequest,
}

pub(crate) fn node_controls(graph_id: &str) -> GraphNodeControls {
    GraphNodeControls {
        graph_id: graph_id.into(),
        revision: 7,
        nodes: vec![NodeControlDescriptor {
            node_id: "effect".into(),
            label: None,
            kind: "gain".into(),
            control: NodeControl::default(),
            gain_db: Some(0.0),
            capabilities: NodeControlCapabilities {
                power: true,
                mute: true,
                recorder: false,
            },
        }],
    }
}

pub(crate) fn control_snapshot() -> mixrelay_control_protocol::EngineControlSnapshot {
    use mixrelay_control_protocol::{
        EngineControlSnapshot, EnginePowerStatus, GraphControlSnapshot, NodeControlSnapshot,
    };
    EngineControlSnapshot {
        revision: 8,
        active_revision: 7,
        structure_generation: 3,
        engine_power: EnginePowerStatus {
            enabled: true,
            transitioning: false,
            unsaved: false,
            pause_reason: None,
        },
        graphs: vec![GraphControlSnapshot {
            id: "graph B".into(),
            name: "Stage".into(),
            nodes: node_controls("graph B")
                .nodes
                .into_iter()
                .map(|descriptor| NodeControlSnapshot {
                    descriptor,
                    recorder: None,
                })
                .collect(),
        }],
    }
}
