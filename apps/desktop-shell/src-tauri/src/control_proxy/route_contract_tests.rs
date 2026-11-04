//! The proxy's routes and receipt rule, checked against the engine's own control router.
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use mixrelay_control_protocol::ErrorCode;
use mixrelay_control_server::{ControlOperations, router};
use mixrelay_desktop_core::engine_control::{
    receipt::{REQUEST_HEADER, requires_client_operation},
    routes,
};
use mixrelay_time::StdClock;
use tower::ServiceExt as _;

/// Marks a path the engine router does not serve, as distinct from a handler's own 404.
const UNROUTED: StatusCode = StatusCode::IM_A_TEAPOT;

/// Every method and route the desktop proxy sends to the engine.
fn proxy_routes() -> Vec<(Method, String)> {
    let id = |route: Result<String, routes::DotSegment>| route.unwrap();
    let node_control = id(routes::node_control("g", "n"));
    vec![
        (Method::GET, routes::STATUS.into()),
        (Method::PUT, routes::AUDIO_SESSION.into()),
        (Method::PUT, routes::AUDIO_POWER.into()),
        (Method::POST, routes::AUDIO_POWER_PREVIEW.into()),
        (Method::PUT, routes::VIRTUAL_ENDPOINT_FORMAT.into()),
        (Method::GET, routes::GRAPH.into()),
        (Method::PUT, routes::GRAPH.into()),
        (Method::GET, routes::GRAPH_STATUS.into()),
        (Method::GET, routes::GRAPH_CATALOG.into()),
        (Method::PUT, routes::GRAPH_CATALOG.into()),
        (Method::GET, routes::CONTROLS_SNAPSHOT.into()),
        (Method::GET, routes::METERS.into()),
        (Method::GET, routes::TELEMETRY.into()),
        (Method::GET, routes::PROCESSORS.into()),
        (Method::PUT, routes::PROCESSOR_RETRY.into()),
        (Method::PUT, routes::PROCESSOR_SCAN.into()),
        (Method::POST, routes::WORKSPACE.into()),
        (Method::POST, routes::WORKSPACE_PREVIEW.into()),
        (Method::GET, routes::CAPTURE_MANIFEST.into()),
        (Method::POST, routes::CAPTURE_PREVIEW.into()),
        (Method::GET, id(routes::graph_layout("g"))),
        (Method::PUT, id(routes::graph_layout("g"))),
        (Method::GET, id(routes::operation("o"))),
        (Method::DELETE, id(routes::operation("o"))),
        (Method::GET, id(routes::node_controls("g"))),
        (Method::POST, routes::preview(&node_control)),
        (Method::PUT, node_control),
        (Method::GET, id(routes::recorder("g", "n"))),
        (Method::PUT, id(routes::recorder("g", "n"))),
        (Method::GET, id(routes::processor_editor("n"))),
        (Method::PUT, id(routes::processor_editor("n"))),
        (Method::GET, id(routes::processor_editor_frame("n"))),
        (Method::POST, id(routes::processor_editor_input("n"))),
        (Method::PUT, id(routes::processor_parameter("n", 7))),
    ]
}

/// Sends one bodyless request to a fresh engine router without an engine behind it.
async fn send(method: &Method, path: &str, operation_id: Option<&str>) -> (StatusCode, Body) {
    let engine =
        router(ControlOperations::new(Box::new(StdClock::new()))).fallback(|| async { UNROUTED });
    let mut request = Request::builder().method(method).uri(path);
    if let Some(id) = operation_id {
        request = request.header(REQUEST_HEADER, id);
    }
    let response = engine
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    (response.status(), response.into_body())
}

#[tokio::test]
async fn the_engine_serves_every_route_the_proxy_sends() {
    for (method, path) in proxy_routes() {
        let (status, _) = send(&method, &path, None).await;
        assert!(
            status != UNROUTED && status != StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path} answered {status}"
        );
    }
}

/// A malformed operation ID is refused only by the engine's operation dispatcher, so the
/// refusal shows which requests the engine tracks as operations.
#[tokio::test]
async fn the_proxy_sends_an_operation_id_exactly_where_the_engine_tracks_one() {
    let refused = serde_json::to_value(ErrorCode::InvalidOperationId).unwrap();
    for (method, path) in proxy_routes() {
        let (status, body) = send(&method, &path, Some("malformed")).await;
        let tracked = status == StatusCode::BAD_REQUEST && {
            let bytes = axum::body::to_bytes(body, 64 * 1024).await.unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"] == refused
        };
        assert_eq!(
            requires_client_operation(method.as_str(), &path),
            tracked,
            "{method} {path}"
        );
    }
}
