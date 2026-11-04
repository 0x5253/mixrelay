use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    body::{Body, Bytes},
    extract::{Path, Request},
    http::{HeaderValue, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tower::ServiceExt;

use super::REQUEST_HEADER;
use crate::operations::{
    INGRESS_PERMITS, MAX_RECEIPTS,
    engine_fence::ENGINE_HEADER,
    ledger::Snapshot,
    receipt_routes,
    responses::{OPERATION_HEADER, RESULT_HEADER},
    test_support::{
        assert_invalid_id, fast_operations, json_body, operations_waiting, pending_id, post_work,
        post_work_as, terminal_response, test_router,
    },
};

#[tokio::test]
async fn client_ids_retain_fast_results_and_deduplicate_even_when_admission_is_closed() {
    let operations = fast_operations();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let router = test_router(
        operations.clone(),
        post(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            async {
                (
                    StatusCode::CONFLICT,
                    Json(json!({"code":"graph_exchange_busy"})),
                )
            }
        }),
    );
    let id = "Client_0123456789-";
    for _ in 0..2 {
        let response = router.clone().oneshot(post_work_as(id)).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[RESULT_HEADER], id);
        assert_eq!(json_body(response).await["code"], "graph_exchange_busy");
        operations.close_admission();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(operations.lock_ledger().len(), 1);
    assert_eq!(
        receipt_routes::delete(Extension(operations.clone()), Path(id.into()))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(matches!(operations.snapshot(id), Snapshot::Missing));
}

#[tokio::test]
async fn duplicate_pending_id_does_not_execute_or_consume_another_slot() {
    let operations = fast_operations();
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let handler_release = release.clone();
    let router = test_router(
        operations.clone(),
        post(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            let release = handler_release.clone();
            async move {
                release.notified().await;
                Json(json!({"ok":true}))
            }
        }),
    );
    let id = "pending-client-01";
    let first = router.clone().oneshot(post_work_as(id)).await.unwrap();
    assert_eq!(pending_id(first).await, id);
    for _ in 1..MAX_RECEIPTS {
        operations.admit(None).unwrap();
    }
    let duplicate = router.oneshot(post_work_as(id)).await.unwrap();
    assert_eq!(pending_id(duplicate).await, id);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(operations.lock_ledger().len(), MAX_RECEIPTS);
    release.notify_one();
    assert_eq!(
        terminal_response(&operations, id).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn existing_ids_resolve_before_body_reads_with_saturated_full_middleware() {
    // Every handler here waits for the test, so callers need not wait before going pending.
    let operations = operations_waiting(Duration::ZERO);
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_release = release.clone();
    let handler_calls = calls.clone();
    let handlers = Router::new().route(
        "/work",
        post(move |body: Bytes| {
            let release = handler_release.clone();
            let calls = handler_calls.clone();
            async move {
                assert_eq!(body, "{\"work\":true}");
                calls.fetch_add(1, Ordering::SeqCst);
                release.acquire().await.unwrap().forget();
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"code":"node_not_found"})),
                )
            }
        }),
    );
    let router = crate::operation_middleware(handlers, operations.clone(), "engine".into());
    saturate_ingress(&router).await;
    assert!(operations.try_acquire_ingress().is_none());
    wait_for_calls(&calls, MAX_RECEIPTS).await;
    // A body that never completes would time out if ingress tried to read it.
    let unread_request = |id: &str| {
        Request::post("/work")
            .header(REQUEST_HEADER, id)
            .body(Body::from_stream(futures_util::stream::pending::<
                Result<Bytes, std::io::Error>,
            >()))
            .unwrap()
    };
    let duplicate = router
        .clone()
        .oneshot(unread_request("client-operation-0"))
        .await
        .unwrap();
    assert_eq!(duplicate.headers()[ENGINE_HEADER], "engine");
    assert_eq!(duplicate.headers()[axum::http::header::CONNECTION], "close");
    assert_eq!(pending_id(duplicate).await, "client-operation-0");
    let new_id = router
        .clone()
        .oneshot(
            Request::post("/work")
                .header(REQUEST_HEADER, "client-operation-new")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(new_id.status(), StatusCode::SERVICE_UNAVAILABLE);
    let invalid = router.clone().oneshot(unread_request("bad")).await.unwrap();
    assert_invalid_id(invalid).await;
    release.add_permits(MAX_RECEIPTS);
    operations.drain().await;
    // Retained terminal errors must also bypass ingress, even while closing.
    let _held: Vec<_> = (0..INGRESS_PERMITS)
        .map(|_| operations.try_acquire_ingress().unwrap())
        .collect();
    operations.close_admission();
    let completed = router
        .oneshot(unread_request("client-operation-0"))
        .await
        .unwrap();
    assert_eq!(completed.status(), StatusCode::NOT_FOUND);
    assert_eq!(completed.headers()[RESULT_HEADER], "client-operation-0");
    assert_eq!(completed.headers()[ENGINE_HEADER], "engine");
    assert_eq!(json_body(completed).await["code"], "node_not_found");
    assert_eq!(calls.load(Ordering::SeqCst), MAX_RECEIPTS);
}

async fn saturate_ingress(router: &Router) {
    for index in 0..MAX_RECEIPTS {
        let response = router
            .clone()
            .oneshot(
                Request::post("/work")
                    .header(REQUEST_HEADER, format!("client-operation-{index}"))
                    .body(Body::from(r#"{"work":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
}

/// Waits until detached handlers have started `expected` times.
async fn wait_for_calls(calls: &AtomicUsize, expected: usize) {
    for _ in 0..200 {
        if calls.load(Ordering::SeqCst) == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), expected);
}

#[tokio::test]
async fn validates_client_ids_before_admission() {
    let operations = fast_operations();
    let router = test_router(
        operations.clone(),
        post(|| async { StatusCode::NO_CONTENT }),
    );
    for id in [
        String::new(),
        "a".repeat(15),
        "a".repeat(65),
        "a".repeat(15) + "/",
        "a".repeat(15) + ".",
    ] {
        let response = router.clone().oneshot(post_work_as(id)).await.unwrap();
        assert_invalid_id(response).await;
    }
    let response = router
        .clone()
        .oneshot(
            Request::post("/work")
                .header(REQUEST_HEADER, HeaderValue::from_bytes(b"\xff").unwrap())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_invalid_id(response).await;
    assert_eq!(operations.lock_ledger().len(), 0);
    for id in ["a".repeat(16), "Z0_-".repeat(16)] {
        let response = router.clone().oneshot(post_work_as(&id)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()[RESULT_HEADER], id);
    }
}

#[tokio::test]
async fn original_accepted_response_has_no_operation_header() {
    let router = test_router(
        fast_operations(),
        post(|| async { (StatusCode::ACCEPTED, Json(json!({"accepted": true}))) }),
    );
    let response = router.oneshot(post_work()).await.expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(response.headers().get(OPERATION_HEADER).is_none());
    assert!(response.headers().get(RESULT_HEADER).is_none());
}

#[tokio::test]
async fn disconnected_waiter_does_not_replay_work() {
    let operations = fast_operations();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_started = started.clone();
    let handler_release = release.clone();
    let handler_calls = calls.clone();
    let router = test_router(
        operations.clone(),
        post(move || async move {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            handler_started.notify_one();
            handler_release.notified().await;
            StatusCode::NO_CONTENT
        }),
    );
    let request = tokio::spawn(router.oneshot(post_work()));
    started.notified().await;
    request.abort();
    release.notify_one();
    for _ in 0..100 {
        if operations.lock_ledger().pending() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(operations.lock_ledger().pending(), 0);
}

#[tokio::test]
async fn quick_responses_reclaim_receipts() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let router = test_router(
        fast_operations(),
        post(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::NO_CONTENT }
        }),
    );
    for _ in 0..(MAX_RECEIPTS * 2) {
        let response = router.clone().oneshot(post_work()).await.expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    assert_eq!(calls.load(Ordering::SeqCst), MAX_RECEIPTS * 2);
}

#[tokio::test]
async fn middleware_rejects_the_ninth_pending_request_before_handler() {
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let handler_started = started.clone();
    let handler_release = release.clone();
    let router = test_router(
        fast_operations(),
        post(move || {
            handler_started.fetch_add(1, Ordering::SeqCst);
            let handler_release = handler_release.clone();
            async move {
                handler_release.notified().await;
                StatusCode::NO_CONTENT
            }
        }),
    );
    let requests: Vec<_> = (0..MAX_RECEIPTS)
        .map(|_| {
            let router = router.clone();
            tokio::spawn(async move { router.oneshot(post_work()).await.expect("response") })
        })
        .collect();
    wait_for_calls(&started, MAX_RECEIPTS).await;
    let response = router
        .oneshot(post_work())
        .await
        .expect("capacity response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = json_body(response).await;
    assert_eq!(body["parameters"]["operationOutcome"], "not_applied");
    for request in requests {
        assert_eq!(
            request.await.expect("request task").status(),
            StatusCode::ACCEPTED
        );
    }
    release.notify_waiters();
}
