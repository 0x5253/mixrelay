//! HTTP adapters and execution ownership for the engine control port.
use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post, put},
};

mod application_capture;
mod audio_session;
mod control_backend;
mod editor_surface;
mod engine_power;
mod engine_status;
mod graph;
mod graph_catalog;
mod ingress;
mod node_control;
mod operations;
mod processors;
mod recorder;
mod reply;
mod telemetry;
mod transport;
mod workspace;

pub use control_backend::ControlBackend;
use control_backend::UnavailableBackend;
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
        .route(STATUS_ROUTE, get(engine_status::status))
        .route(AUDIO_SESSION_ROUTE, put(audio_session::reconfigure))
        .route("/api/v1/audio/power", put(engine_power::set))
        .route("/api/v1/audio/power/preview", post(engine_power::preview))
        .route(
            VIRTUAL_ENDPOINT_FORMAT_ROUTE,
            put(audio_session::reconfigure_virtual_endpoint),
        )
        .route(GRAPH_STATUS_ROUTE, get(graph::status))
        .route(CONTROL_SNAPSHOT_ROUTE, get(node_control::snapshot))
        .route(METERS_ROUTE, get(engine_status::meters))
        .route(CONTROL_TELEMETRY_ROUTE, get(telemetry::subscribe))
        .route(
            APPLICATION_CAPTURE_MANIFEST_ROUTE,
            get(application_capture::manifest),
        )
        .route(
            APPLICATION_CAPTURE_PREVIEW_ROUTE,
            post(application_capture::preview),
        )
        .route(PROCESSORS_ROUTE, get(processors::catalog))
        .route(PROCESSORS_SCAN_ROUTE, put(processors::rescan))
        .route(PROCESSOR_PARAMETER_ROUTE, put(processors::set_parameter))
        .route(PROCESSOR_RETRY_ROUTE, put(processors::retry_failed))
        .route(
            PROCESSOR_EDITOR_ROUTE,
            get(processors::editor_status).put(processors::set_editor),
        )
        .route(GRAPH_ROUTE, get(graph::document).put(graph::apply))
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
            get(graph_catalog::catalog).put(graph_catalog::mutate),
        )
        .route(GRAPH_CATALOG_DOCUMENT_ROUTE, get(graph_catalog::document))
        .route(
            GRAPH_LAYOUT_ROUTE,
            get(graph_catalog::layout).put(graph_catalog::update_layout),
        )
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
