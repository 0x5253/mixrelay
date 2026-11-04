use super::*;
use std::time::Duration;

/// A client of a socket no engine listens on; nothing it admits can reach live audio.
fn unreachable_engine() -> (mixrelay_test_support::FixtureDir, ControlProxyClient) {
    let fixture = mixrelay_test_support::FixtureDir::ipc("adm").expect("fixture");
    let client = ControlProxyClient::with_socket(Some(fixture.join("engine.sock")));
    (fixture, client)
}

#[tokio::test]
async fn without_an_engine_socket_nothing_is_sent() {
    let client = ControlProxyClient::with_socket(None);
    let rejected = client
        .request(Method::GET, routes::STATUS, None)
        .await
        .expect("rejected locally");
    assert_eq!(rejected.status, 409);
    assert_eq!(rejected.body["code"], "control_unavailable");
    assert!(client.meter_stream().await.is_err());
}

#[tokio::test]
async fn a_suspended_client_refuses_requests_until_recovery_but_shutdown_is_final() {
    let (_fixture, client) = unreachable_engine();
    client.suspend_admission();
    let refused = client
        .request(Method::GET, routes::STATUS, None)
        .await
        .expect("refused locally");
    assert_eq!(refused.status, 409);
    assert!(client.resume_admission().await);
    // Admitted again: with no engine listening the request now fails to connect instead.
    let unreachable = client
        .request(Method::GET, routes::STATUS, None)
        .await
        .expect("connection failure");
    assert_eq!(unreachable.status, 503);
    client.close_admission();
    assert!(!client.resume_admission().await);
    let closed = client
        .request(Method::GET, routes::STATUS, None)
        .await
        .expect("shutdown remains closed");
    assert_eq!(closed.status, 409);
}

#[tokio::test(start_paused = true)]
async fn live_receipts_extend_drain_without_releasing_accepted_ownership() {
    let (_fixture, client) = unreachable_engine();
    let accepted = client.admission.enter().await.expect("admitted");
    client.close_admission();
    let draining = tokio::spawn({
        let client = client.clone();
        async move { client.drain().await }
    });
    tokio::task::yield_now().await;
    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(!draining.is_finished());
        client.operation_liveness.notify_waiters();
        tokio::task::yield_now().await;
    }
    drop(accepted);
    draining
        .await
        .expect("drain task")
        .expect("live request drained");
}

#[tokio::test(start_paused = true)]
async fn missing_liveness_reports_retained_ownership_without_cancelling_request() {
    let (_fixture, client) = unreachable_engine();
    let accepted = client.admission.enter().await.expect("admitted");
    let draining = tokio::spawn({
        let client = client.clone();
        async move { client.drain().await }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(36)).await;
    assert!(draining.await.expect("drain task").is_err());
    // The admitted request still holds admission.
    assert!(
        tokio::time::timeout(Duration::from_secs(1), client.admission.quiesce())
            .await
            .is_err()
    );
    drop(accepted);
    client.drain().await.expect("later completion can drain");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn closing_rejects_requests_and_waits_for_accepted_work() {
    let (_fixture, client) = unreachable_engine();
    let accepted = client.admission.enter().await.expect("admitted");
    client.close_admission();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), client.drain())
            .await
            .is_err()
    );
    drop(accepted);
    client.drain().await.expect("drained");
    let response = client
        .request(Method::PUT, "/not-sent", None)
        .await
        .expect("response");
    assert_eq!(
        response.delivery,
        mixrelay_desktop_core::engine_control::Delivery::NotSent
    );
    assert_eq!(response.status, 409);
}

#[test]
fn processor_retry_proxy_preserves_opaque_identity_and_revision_contract() {
    let body = processor_retry_body(&serde_json::json!({
        "expectedCatalogRevision": 6,
        "expectedInstanceId": "18446744073709551615",
        "expectedRevision": 3,
        "graphId": "graph-a",
        "nodeId": "provider-a",
    }))
    .expect("request validates");
    assert_eq!(
        body,
        serde_json::json!({
            "expectedCatalogRevision": 6,
            "expectedInstanceId": "18446744073709551615",
            "expectedRevision": 3,
            "graphId": "graph-a",
            "nodeId": "provider-a",
        })
    );
    for invalid in [
        serde_json::json!({"graphId": "graph-a"}),
        serde_json::json!({
            "expectedCatalogRevision": 6, "expectedInstanceId": "1", "expectedRevision": 3,
            "graphId": "graph-a", "nodeId": "provider-a", "path": "/private",
        }),
    ] {
        assert!(processor_retry_body(&invalid).is_err());
    }
}

#[test]
fn processor_retry_keeps_the_editors_operation_id_for_receipt_resolution() {
    let body = processor_retry_body(&serde_json::json!({
        "clientOperationId": "ui-retry",
        "expectedCatalogRevision": 6,
        "expectedInstanceId": "1",
        "expectedRevision": 3,
        "graphId": "graph-a",
        "nodeId": "provider-a",
    }))
    .expect("the editor's retry request validates");
    assert_eq!(body["clientOperationId"], "ui-retry");
    assert_eq!(body["expectedRevision"], 3);
}
