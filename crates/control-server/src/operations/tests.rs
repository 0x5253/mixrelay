//! Receipt ownership: identities, capacity, acknowledgement and expiry.
use std::time::Duration;

use axum::{http::StatusCode, routing::post};
use mixrelay_time::ManualClock;
use tower::ServiceExt;

use super::{
    COMPLETED_TTL, ControlOperations, MAX_RECEIPTS,
    ledger::{Ack, Snapshot},
    responses::RESULT_HEADER,
    test_support::{fast_operations, post_work_as, stored, test_router},
};

#[test]
fn generated_ids_cannot_overwrite_client_records_and_ack_allows_reuse() {
    let operations = fast_operations();
    let id = format!("op-{:032x}-{:016x}", operations.inner.owner_nonce, 1);
    let client = operations.admit(Some(&id)).unwrap();
    let generated = operations.admit(None).unwrap();
    assert_ne!(client.id, generated.id);
    assert_eq!(operations.lock_ledger().len(), 2);
    operations.complete(&client.id, stored(StatusCode::OK, b"ok", true));
    assert!(matches!(operations.acknowledge(&client.id), Ack::Completed));
    assert_eq!(operations.admit(Some(&id)).unwrap().id, id);
}

#[tokio::test]
async fn client_receipts_are_bounded_until_acknowledged_or_expired() {
    let clock = ManualClock::new(mixrelay_time::Instant::from_origin(Duration::ZERO));
    let operations = ControlOperations::new(Box::new(clock.clone()));
    let router = test_router(
        operations.clone(),
        post(|| async { StatusCode::NO_CONTENT }),
    );
    let first = "client-operation-0";
    for sequence in 0..=MAX_RECEIPTS {
        let response = router
            .clone()
            .oneshot(post_work_as(format!("client-operation-{sequence}")))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if sequence < MAX_RECEIPTS {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
    }
    let duplicate = router.clone().oneshot(post_work_as(first)).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::NO_CONTENT);
    assert_eq!(duplicate.headers()[RESULT_HEADER], first);
    assert!(matches!(operations.acknowledge(first), Ack::Completed));
    operations.admit(Some(first)).unwrap();
    operations.complete(first, stored(StatusCode::OK, b"ok", true));
    clock.advance(COMPLETED_TTL);
    assert_eq!(operations.admit(Some(first)).unwrap().id, first);
}

#[test]
fn ids_change_between_owners_and_requests() {
    let first = fast_operations();
    let second = fast_operations();
    let first_id = first.admit(None).expect("first admission").id;
    let second_id = second.admit(None).expect("second admission").id;
    assert_ne!(first_id, second_id);
}

#[test]
fn admission_is_bounded_without_eviction_of_pending_work() {
    let operations = fast_operations();
    for _ in 0..MAX_RECEIPTS {
        operations.admit(None).expect("bounded admission");
    }
    assert!(operations.admit(None).is_err());
}

#[test]
fn acknowledgement_and_expiry_remove_completed_receipts_only() {
    let clock = ManualClock::new(mixrelay_time::Instant::from_origin(Duration::from_mins(10)));
    let operations = ControlOperations::new(Box::new(clock.clone()));
    let pending = operations.admit(None).expect("pending admission");
    assert!(matches!(operations.acknowledge(&pending.id), Ack::Pending));
    operations.complete(&pending.id, stored(StatusCode::OK, b"ok", true));
    assert!(matches!(
        operations.acknowledge(&pending.id),
        Ack::Completed
    ));
    assert!(matches!(
        operations.snapshot(&pending.id),
        Snapshot::Missing
    ));

    let expired = operations.admit(None).expect("expiry admission");
    clock.advance(COMPLETED_TTL);
    assert!(matches!(
        operations.snapshot(&expired.id),
        Snapshot::Pending
    ));
    operations.complete(&expired.id, stored(StatusCode::OK, b"ok", true));
    clock.advance(COMPLETED_TTL.checked_sub(Duration::from_nanos(1)).unwrap());
    assert!(matches!(
        operations.snapshot(&expired.id),
        Snapshot::Complete(_)
    ));
    clock.advance(Duration::from_nanos(1));
    assert!(matches!(
        operations.snapshot(&expired.id),
        Snapshot::Missing
    ));
    // A late completion for a receipt that is gone changes nothing.
    operations.complete(&expired.id, stored(StatusCode::OK, b"late", true));
    assert_eq!(operations.lock_ledger().len(), 0);
}
