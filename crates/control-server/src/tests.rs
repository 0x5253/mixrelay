//! Router composition without an engine.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use crate::{
    ControlOperations, GRAPH_ROUTE, GRAPH_STATUS_ROUTE, HEALTH_ROUTE, STATUS_ROUTE, router,
    test_support::{apply_request, clock, document, response_json},
};

#[tokio::test]
async fn default_routes_are_loopback_safe_and_versioned() {
    let health = router(ControlOperations::new(clock()))
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
        router(ControlOperations::new(clock())),
        Request::builder()
            .uri(STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(status["driverState"], "not_installed");

    let (_, graph) = response_json(
        router(ControlOperations::new(clock())),
        Request::builder()
            .uri(GRAPH_STATUS_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph["activeRevision"], 0);

    let (_, graph_document) = response_json(
        router(ControlOperations::new(clock())),
        Request::builder()
            .uri(GRAPH_ROUTE)
            .body(Body::empty())
            .expect("request should build"),
    )
    .await;
    assert_eq!(graph_document["revision"], 0);

    let (status, error) = apply_request(router(ControlOperations::new(clock())), &document()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "control_unavailable");
}
