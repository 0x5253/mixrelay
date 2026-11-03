//! Behavioral fixtures for the adjacent native owner.

use super::*;

#[test]
fn ordinary_parameter_invalidation_refreshes_without_restart() {
    let mut snapshot = Some(64);
    let calls = std::cell::Cell::new(0);
    let published_revision = std::cell::Cell::new(None);
    assert_eq!(latency_service_action(false), LatencyServiceAction::Refresh);

    let published = refresh_latency_cache(
        &mut snapshot,
        2,
        || {
            calls.set(calls.get() + 1);
            96
        },
        |revision| {
            published_revision.set(Some(revision));
            true
        },
    );

    assert!(published);
    assert_eq!(snapshot, Some(96));
    assert_eq!(published_revision.get(), Some(2));
    assert_eq!(calls.get(), 1);
}

#[test]
fn native_state_invalidation_refreshes_without_restart() {
    let mut snapshot = None;
    let published_revision = std::cell::Cell::new(None);

    let published = refresh_latency_cache(
        &mut snapshot,
        7,
        || 128,
        |revision| {
            published_revision.set(Some(revision));
            true
        },
    );

    assert!(published);
    assert_eq!(snapshot, Some(128));
    assert_eq!(published_revision.get(), Some(7));
}

#[test]
fn notification_during_latency_query_keeps_snapshot_unknown() {
    let mut snapshot = Some(32);
    let revision = std::cell::Cell::new(10);
    let publish_called = std::cell::Cell::new(false);

    let published = refresh_latency_cache(
        &mut snapshot,
        revision.get(),
        || {
            revision.set(11);
            64
        },
        |captured| {
            let current = revision.get();
            if !latency_revision_is_current(captured, current, false) {
                return false;
            }
            publish_called.set(true);
            true
        },
    );

    assert!(!published);
    assert_eq!(snapshot, None);
    assert!(!publish_called.get());
}

#[test]
fn pending_parameter_delivery_blocks_refresh_and_latency_flags_restart() {
    assert!(!latency_refresh_allowed(true, false, true));
    assert!(latency_refresh_allowed(true, false, false));
    assert!(!latency_refresh_allowed(false, false, false));
    assert!(!latency_refresh_allowed(true, true, false));
    assert_eq!(latency_service_action(true), LatencyServiceAction::Restart);
}
