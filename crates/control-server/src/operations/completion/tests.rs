use std::{sync::Arc, time::Duration};

use axum::{Extension, Json, body::Body, extract::Path, http::StatusCode, routing::post};
use serde_json::json;
use tokio::sync::Notify;
use tower::ServiceExt;

use super::RESPONSE_LIMIT;
use crate::operations::{
    ControlOperations, receipt_routes,
    responses::RESULT_HEADER,
    test_support::{
        json_body, operations_waiting, pending_id, post_work, terminal_response, test_router,
    },
};

/// Every handler here waits for the test, so callers go pending without waiting.
fn gated_operations() -> ControlOperations {
    operations_waiting(Duration::ZERO)
}

#[tokio::test]
async fn slow_success_is_polled_once_and_acknowledged() {
    let operations = gated_operations();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let handler_started = started.clone();
    let handler_release = release.clone();
    let router = test_router(
        operations.clone(),
        post(move || async move {
            handler_started.notify_one();
            handler_release.notified().await;
            Json(json!({"ok": true}))
        }),
    );
    let request = tokio::spawn(router.oneshot(post_work()));
    started.notified().await;
    let id = pending_id(request.await.expect("request task").expect("response")).await;
    release.notify_one();
    let response = terminal_response(&operations, &id).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(RESULT_HEADER).unwrap(), id.as_str());
    let acknowledged =
        receipt_routes::delete(Extension(operations.clone()), Path(id.clone())).await;
    assert_eq!(acknowledged.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        receipt_routes::get(Extension(operations), Path(id))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn slow_original_error_keeps_result_identity() {
    let operations = gated_operations();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let handler_started = started.clone();
    let handler_release = release.clone();
    let router = test_router(
        operations.clone(),
        post(move || async move {
            handler_started.notify_one();
            handler_release.notified().await;
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"code": "graph_invalid"})),
            )
        }),
    );
    let request = tokio::spawn(router.oneshot(post_work()));
    started.notified().await;
    let id = pending_id(request.await.expect("request task").expect("response")).await;
    release.notify_one();
    let response = terminal_response(&operations, &id).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.headers().get(RESULT_HEADER).unwrap(), id.as_str());
}

#[tokio::test]
async fn blocking_task_panic_is_uncertain_without_result_header() {
    let operations = gated_operations();
    let release = Arc::new(Notify::new());
    let handler_release = release.clone();
    let router = test_router(
        operations.clone(),
        post(move || async move {
            handler_release.notified().await;
            crate::reply::catalog_task(|| panic!("fixture blocking task panic")).await
        }),
    );
    let id = pending_id(router.oneshot(post_work()).await.expect("response")).await;
    release.notify_one();
    let response = terminal_response(&operations, &id).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get(RESULT_HEADER).is_none());
    assert_eq!(
        json_body(response).await["parameters"]["operationOutcome"],
        "uncertain"
    );
}

#[tokio::test]
async fn panic_and_oversize_are_uncertain_without_result_header() {
    let operations = gated_operations();
    let release = Arc::new(Notify::new());
    let handler_release = release.clone();
    let router = test_router(
        operations.clone(),
        post(move || async move {
            handler_release.notified().await;
            panic_status()
        }),
    );
    let id = pending_id(router.oneshot(post_work()).await.expect("response")).await;
    release.notify_one();
    let response = terminal_response(&operations, &id).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().get(RESULT_HEADER).is_none());
    assert_eq!(
        json_body(response).await["parameters"]["operationOutcome"],
        "uncertain"
    );

    let operations = gated_operations();
    let release = Arc::new(Notify::new());
    let handler_release = release.clone();
    let router = test_router(
        operations.clone(),
        post(move || async move {
            handler_release.notified().await;
            Body::from(vec![b'x'; RESPONSE_LIMIT + 1])
        }),
    );
    let id = pending_id(router.oneshot(post_work()).await.expect("response")).await;
    release.notify_one();
    let response = terminal_response(&operations, &id).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().get(RESULT_HEADER).is_none());
}

fn panic_status() -> StatusCode {
    panic!("handler panic");
}
