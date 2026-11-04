//! HTTP adapters and execution ownership for the engine control port.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use mixrelay_audio_graph::GraphDocument;
use mixrelay_control_protocol::{
    AppStatus, ErrorCode, ExternalProcessorCatalog, GraphApplyRequest, GraphApplyResponse,
    GraphCatalogRequest, GraphLayoutRequest, GraphRuntimeStatus, MeterSnapshot, PROTOCOL_VERSION,
    ProcessorEditorRequest, ProcessorEditorStatus, ProcessorParameterUpdate,
    ProcessorParameterUpdateResponse, ProcessorRetryRequest, ProcessorScanRequest, ProtocolError,
    SessionReconfigureRequest,
};
mod control_backend;
mod editor_surface;
mod engine_power;
mod ingress;
mod node_control;
mod operations;
mod recorder;
mod telemetry;
mod transport;
mod workspace;
pub use control_backend::ControlBackend;
use control_backend::processor_unavailable;
pub use operations::{ControlOperations, OperationTimings};
pub use transport::{SocketLimits, bounded_listener};

pub const HEALTH_ROUTE: &str = "/health";
pub const STATUS_ROUTE: &str = "/api/v1/status";
pub const AUDIO_SESSION_ROUTE: &str = "/api/v1/audio/session";
pub const VIRTUAL_ENDPOINT_FORMAT_ROUTE: &str = "/api/v1/audio/virtual-endpoint/format";
pub const GRAPH_STATUS_ROUTE: &str = "/api/v1/graph/status";
/// Coherent read-side control catalog, independent of graph mutation admission.
pub const CONTROL_SNAPSHOT_ROUTE: &str = "/api/v1/controls/snapshot";
pub const GRAPH_ROUTE: &str = "/api/v1/graph";
pub const GRAPH_CATALOG_ROUTE: &str = "/api/v1/graphs";
pub const GRAPH_CATALOG_DOCUMENT_ROUTE: &str = "/api/v1/graphs/{id}";
pub const GRAPH_LAYOUT_ROUTE: &str = "/api/v1/graphs/{id}/layout";
pub const METERS_ROUTE: &str = "/api/v1/meters";
/// Server-sent meter windows, one per publication window; a push alternative to polling.
pub const CONTROL_TELEMETRY_ROUTE: &str = "/api/v1/telemetry";
pub const METER_TELEMETRY_ROUTE: &str = CONTROL_TELEMETRY_ROUTE;
/// The publication window pushed meters follow.
const METER_TELEMETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
pub const APPLICATION_CAPTURE_MANIFEST_ROUTE: &str = "/api/v1/application-capture/manifest";
pub const APPLICATION_CAPTURE_PREVIEW_ROUTE: &str = "/api/v1/application-capture/preview";
pub const PROCESSORS_ROUTE: &str = "/api/v1/processors";
pub const PROCESSORS_SCAN_ROUTE: &str = "/api/v1/processors/scan";
pub const PROCESSOR_PARAMETER_ROUTE: &str =
    "/api/v1/processors/{node_id}/parameters/{parameter_id}";
pub const PROCESSOR_EDITOR_ROUTE: &str = "/api/v1/processors/{node_id}/editor";
pub const PROCESSOR_RETRY_ROUTE: &str = "/api/v1/processors/retry";

#[derive(Clone)]
struct ControlState {
    telemetry: Arc<telemetry::Publisher>,
    backend: Arc<dyn ControlBackend>,
}

#[derive(Debug, Default)]
struct UnavailableBackend;

impl ControlBackend for UnavailableBackend {
    fn status(&self) -> AppStatus {
        AppStatus::default()
    }

    fn graph_status(&self) -> GraphRuntimeStatus {
        GraphRuntimeStatus {
            pending_reason: None,
            protocol_version: PROTOCOL_VERSION,
            active_revision: 0,
            submitted_revision: 0,
        }
    }

    fn graph(&self) -> GraphDocument {
        GraphDocument::new(0, vec![], vec![])
    }

    fn meters(&self) -> MeterSnapshot {
        MeterSnapshot {
            protocol_version: PROTOCOL_VERSION,
            revision: 0,
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
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({}),
        })
    }

    fn set_processor_parameter(
        &self,
        node_id: &str,
        parameter_id: u32,
        _update: ProcessorParameterUpdate,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ProcessorUnavailable,
            parameters: serde_json::json!({
                "nodeId": node_id,
                "parameterId": parameter_id,
            }),
        })
    }

    fn processor_editor_status(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        Err(processor_unavailable(node_id))
    }

    fn set_processor_editor(
        &self,
        node_id: &str,
        _request: ProcessorEditorRequest,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        Err(processor_unavailable(node_id))
    }

    fn retry_failed_processor(
        &self,
        _request: ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"processor_retry"}),
        })
    }

    fn apply_graph_request(
        &self,
        _request: GraphApplyRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({}),
        })
    }
}

pub fn router(operations: ControlOperations) -> Router {
    router_with_backend(Arc::new(UnavailableBackend), operations)
}

pub fn router_with_backend(
    backend: Arc<dyn ControlBackend>,
    operations: ControlOperations,
) -> Router {
    operation_middleware(control_routes(), operations, backend.incarnation()).with_state(
        ControlState {
            backend,
            telemetry: Arc::default(),
        },
    )
}

fn operation_middleware<S: Clone + Send + Sync + 'static>(
    routes: Router<S>,
    operations: ControlOperations,
    incarnation: String,
) -> Router<S> {
    routes
        // The last layer is outermost in axum. Keep body admission outside the operation
        // dispatcher so malformed/incomplete bodies never consume an operation receipt.
        // Ingress resolves existing client IDs before acquiring a body permit.
        .layer(axum::middleware::from_fn(operations::dispatch))
        .layer(axum::middleware::from_fn(ingress::read_body))
        // Ingress already bounds every body; avoid Json's separate 2 MiB default.
        .layer(DefaultBodyLimit::disable())
        .layer(axum::Extension(operations))
        .layer(axum::middleware::from_fn_with_state(
            operations::engine_header(incarnation),
            operations::engine_identity,
        ))
}

fn control_routes() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/operations/{id}",
            get(operations::get).delete(operations::delete),
        )
        .route(
            "/api/v1/graphs/{id}/nodes/controls",
            get(node_control::list),
        )
        .route(
            "/api/v1/graphs/{id}/nodes/{node_id}/control",
            put(node_control::update),
        )
        .route(
            "/api/v1/graphs/{id}/nodes/{node_id}/control/preview",
            post(node_control::preview),
        )
        .route("/api/v1/workspace", post(workspace::operate))
        .route("/api/v1/workspace/preview", post(workspace::preview))
        .route(
            "/api/v1/graphs/{id}/recorders/{node_id}",
            get(recorder::status).put(recorder::control),
        )
        .route(HEALTH_ROUTE, get(|| async { "ok" }))
        .route(STATUS_ROUTE, get(status))
        .route(AUDIO_SESSION_ROUTE, put(reconfigure_session))
        .route("/api/v1/audio/power", put(engine_power::set))
        .route("/api/v1/audio/power/preview", post(engine_power::preview))
        .route(
            VIRTUAL_ENDPOINT_FORMAT_ROUTE,
            put(reconfigure_virtual_endpoint),
        )
        .route(GRAPH_STATUS_ROUTE, get(graph_status))
        .route(CONTROL_SNAPSHOT_ROUTE, get(node_control::snapshot))
        .route(METERS_ROUTE, get(meters))
        .route(CONTROL_TELEMETRY_ROUTE, get(telemetry::subscribe))
        .route(
            APPLICATION_CAPTURE_MANIFEST_ROUTE,
            get(application_capture_manifest),
        )
        .route(
            APPLICATION_CAPTURE_PREVIEW_ROUTE,
            post(preview_application_capture),
        )
        .route(PROCESSORS_ROUTE, get(processors))
        .route(PROCESSORS_SCAN_ROUTE, put(rescan_processors))
        .route(PROCESSOR_PARAMETER_ROUTE, put(set_processor_parameter))
        .route(PROCESSOR_RETRY_ROUTE, put(retry_failed_processor))
        .route(
            PROCESSOR_EDITOR_ROUTE,
            get(processor_editor_status).put(set_processor_editor),
        )
        .route(GRAPH_ROUTE, get(graph).put(apply_graph))
        .route(
            "/api/v1/processors/{node_id}/editor/frame",
            get(editor_surface::frame),
        )
        .route(
            "/api/v1/processors/{node_id}/editor/input",
            post(editor_surface::input),
        )
        .route(
            GRAPH_CATALOG_ROUTE,
            get(graph_catalog).put(mutate_graph_catalog),
        )
        .route(GRAPH_CATALOG_DOCUMENT_ROUTE, get(graph_catalog_document))
        .route(
            GRAPH_LAYOUT_ROUTE,
            get(graph_layout).put(update_graph_layout),
        )
}

async fn status(State(state): State<ControlState>) -> Response {
    catalog_task(move || Json(state.backend.status()).into_response()).await
}

async fn graph_status(State(state): State<ControlState>) -> Json<GraphRuntimeStatus> {
    Json(state.backend.graph_status())
}

async fn graph(State(state): State<ControlState>) -> Json<GraphDocument> {
    Json(state.backend.graph())
}

async fn application_capture_manifest(State(state): State<ControlState>) -> Response {
    catalog_task(move || match state.backend.application_capture_manifest() {
        Ok(response) => Json(response).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    })
    .await
}

async fn preview_application_capture(
    State(state): State<ControlState>,
    request: Result<Json<GraphApplyRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({"reason": rejection.body_text()}),
                }),
            )
                .into_response();
        }
    };
    catalog_task(
        move || match state.backend.preview_application_capture(request) {
            Ok(response) => Json(response).into_response(),
            Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
        },
    )
    .await
}

async fn graph_catalog(State(state): State<ControlState>) -> Response {
    catalog_task(move || match state.backend.graph_catalog() {
        Ok(response) => Json(response).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    })
    .await
}

async fn graph_layout(State(state): State<ControlState>, Path(id): Path<String>) -> Response {
    catalog_task(move || match state.backend.graph_layout(&id) {
        Ok(response) => Json(response).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    })
    .await
}

async fn update_graph_layout(
    State(state): State<ControlState>,
    Path(id): Path<String>,
    request: Result<Json<GraphLayoutRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(rejection) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(ProtocolError {
            code: ErrorCode::GraphInvalid, parameters: serde_json::json!({"resource":"graph_layout","graphId":id,"reason":rejection.body_text()}),
        })).into_response(),
    };
    catalog_task(
        move || match state.backend.update_graph_layout(&id, request) {
            Ok(response) => Json(response).into_response(),
            Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
        },
    )
    .await
}

async fn graph_catalog_document(
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> Response {
    catalog_task(move || match state.backend.graph_catalog_document(&id) {
        Ok(response) => Json(response).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    })
    .await
}

async fn mutate_graph_catalog(
    State(state): State<ControlState>,
    request: Result<Json<GraphCatalogRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({"reason": error.body_text()}),
                }),
            )
                .into_response();
        }
    };
    catalog_task(move || match state.backend.mutate_graph_catalog(request) {
        Ok(response) => Json(response).into_response(),
        Err(mut error) => {
            error.parameters["catalogOutcome"] = serde_json::json!("not_applied");
            (protocol_error_status(error.code), Json(error)).into_response()
        }
    })
    .await
}

// Catalog reads may wait behind a native snapshot and writes perform filesystem I/O.
// Neither may block a Tokio reactor, even when there is only one asynchronous worker.
async fn catalog_task(work: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|_| {
        operations::task_failure(
            ErrorCode::ControlUnavailable,
            "graph catalog control task failed",
        )
    })
}

async fn meters(State(state): State<ControlState>) -> Json<MeterSnapshot> {
    Json(state.backend.meters())
}

async fn reconfigure_session(
    State(state): State<ControlState>,
    request: Result<Json<SessionReconfigureRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::AudioSessionUnsupported,
                    parameters: serde_json::json!({"reason": error.body_text()}),
                }),
            )
                .into_response();
        }
    };
    // Driver negotiation and plug-in preparation can block. Do not occupy a Tokio I/O worker.
    match tokio::task::spawn_blocking(move || state.backend.reconfigure_session(request)).await {
        Ok(Ok(response)) => (StatusCode::ACCEPTED, Json(response)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => operations::task_failure(
            ErrorCode::AudioSessionReconfigureFailed,
            "session control task failed",
        ),
    }
}

async fn processors(State(state): State<ControlState>) -> Json<ExternalProcessorCatalog> {
    Json(state.backend.processors())
}

async fn reconfigure_virtual_endpoint(
    State(state): State<ControlState>,
    request: Result<Json<mixrelay_control_protocol::VirtualEndpointFormatRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::AudioSessionUnsupported,
                    parameters: serde_json::json!({"reason": error.body_text()}),
                }),
            )
                .into_response();
        }
    };
    match tokio::task::spawn_blocking(move || state.backend.reconfigure_virtual_endpoint(request))
        .await
    {
        Ok(Ok(response)) => (StatusCode::ACCEPTED, Json(response)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => operations::task_failure(
            ErrorCode::AudioSessionReconfigureFailed,
            "virtual endpoint control task failed",
        ),
    }
}

async fn rescan_processors(
    State(state): State<ControlState>,
    request: Result<Json<ProcessorScanRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({ "reason": rejection.body_text() }),
                }),
            )
                .into_response();
        }
    };
    match state.backend.rescan_processors(request) {
        Ok(catalog) => (StatusCode::OK, Json(catalog)).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    }
}

async fn set_processor_parameter(
    State(state): State<ControlState>,
    Path((node_id, parameter_id)): Path<(String, u32)>,
    update: Result<Json<ProcessorParameterUpdate>, JsonRejection>,
) -> Response {
    let Json(update) = match update {
        Ok(update) => update,
        Err(rejection) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({ "reason": rejection.body_text() }),
                }),
            )
                .into_response();
        }
    };
    // Accepted parameter updates also checkpoint durable graph state. Keep filesystem work
    // off Tokio's I/O workers; the backend still publishes the native value before saving.
    match tokio::task::spawn_blocking(move || {
        state
            .backend
            .set_processor_parameter(&node_id, parameter_id, update)
    })
    .await
    {
        Ok(Ok(response)) => (StatusCode::OK, Json(response)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => operations::task_failure(
            ErrorCode::ControlUnavailable,
            "processor parameter control task failed",
        ),
    }
}

async fn processor_editor_status(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
) -> Response {
    match state.backend.processor_editor_status(&node_id) {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    }
}

async fn set_processor_editor(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
    request: Result<Json<ProcessorEditorRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({ "reason": rejection.body_text() }),
                }),
            )
                .into_response();
        }
    };
    // Closing includes a bounded vendor state export and durable filesystem write.
    // Do not stall meter/status HTTP requests on the asynchronous I/O executor.
    match tokio::task::spawn_blocking(move || state.backend.set_processor_editor(&node_id, request))
        .await
    {
        Ok(Ok(response)) => (StatusCode::OK, Json(response)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => operations::task_failure(
            ErrorCode::ControlUnavailable,
            "processor editor control task failed",
        ),
    }
}

async fn retry_failed_processor(
    State(state): State<ControlState>,
    request: Result<Json<ProcessorRetryRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({ "reason": rejection.body_text() }),
                }),
            )
                .into_response();
        }
    };
    match tokio::task::spawn_blocking(move || state.backend.retry_failed_processor(request)).await {
        Ok(Ok(response)) => (StatusCode::OK, Json(response)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => {
            operations::task_failure(ErrorCode::ControlUnavailable, "processor retry task failed")
        }
    }
}

async fn apply_graph(
    State(state): State<ControlState>,
    document: Result<Json<GraphApplyRequest>, JsonRejection>,
) -> Response {
    let Json(document) = match document {
        Ok(document) => document,
        Err(rejection) => {
            let error = ProtocolError {
                code: ErrorCode::GraphInvalid,
                parameters: serde_json::json!({ "reason": rejection.body_text() }),
            };
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(error)).into_response();
        }
    };
    match tokio::task::spawn_blocking(move || state.backend.apply_graph_request(document)).await {
        Ok(Ok(response)) => (StatusCode::ACCEPTED, Json(response)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => {
            operations::task_failure(ErrorCode::ControlUnavailable, "graph control task failed")
        }
    }
}

fn protocol_error_status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidOperationId => StatusCode::BAD_REQUEST,
        ErrorCode::GraphRevisionStale
        | ErrorCode::GraphExchangeBusy
        | ErrorCode::NodeDisabled
        | ErrorCode::EngineReplaced => StatusCode::CONFLICT,
        ErrorCode::NodeNotFound => StatusCode::NOT_FOUND,
        ErrorCode::GraphInvalid
        | ErrorCode::GraphBindingUnsupported
        | ErrorCode::AudioSessionUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Unauthorized | ErrorCode::PairingRequired => StatusCode::UNAUTHORIZED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests;
