//! Cold first admission, reusable storage and cancellation at the endpoint control boundary.
mod support;
use super::*;
use std::{cell::Cell, rc::Rc};
use support::*;

fn assert_cold_first_admission(rate: u32) {
    let fixture = Fixture::new(rate, 2);
    fixture.prepare();
    fixture.admit();
    // A delay left in admission lets HAL write past its 65,536-frame queue.
    fixture.play(rate * 4);
    fixture.verify();
    assert_eq!(fixture.native.borrow().allocations, 1);
}

#[test]
fn cold_first_admission_keeps_native_sample_zero_at_48k() {
    assert_cold_first_admission(48_000);
}

#[test]
fn cold_first_admission_keeps_native_sample_zero_at_192k() {
    assert_cold_first_admission(192_000);
}

#[test]
fn empty_client_snapshot_reuses_storage_and_re_admits_from_the_new_sample_zero() {
    let fixture = Fixture::new(192_000, 2);
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    fixture.verify();
    fixture
        .owner
        .replace(Vec::new(), host_time(), host_time())
        .unwrap();
    fixture.update();
    assert_eq!(
        fixture
            .owner
            .sources
            .pcm_progress(token())
            .unwrap()
            .clock
            .running,
        0
    );
    fixture.native.borrow_mut().running = false;
    fixture.prepare();
    let admission = allocation_counter::measure(|| fixture.admit());
    // Queue handoff owns control metadata, never another retention-sized PCM store.
    assert!(
        admission.bytes_total < u64::from(QUEUE_FRAMES) * 4,
        "admission allocated {} bytes",
        admission.bytes_total
    );
    fixture.play(1_024);
    fixture.verify();
    assert_eq!(fixture.native.borrow().allocations, 1);
}

#[test]
fn coalesced_empty_membership_discards_the_ended_sessions_pcm() {
    let fixture = Fixture::new(48_000, 2);
    fixture.prepare();
    fixture.native.borrow_mut().start();
    fixture
        .owner
        .replace(fixture.native.borrow().queues(), host_time(), host_time())
        .unwrap();
    fixture.update();
    assert!(fixture.owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Routed { head: true }
    ));
    // No format has been requested, so all of this session remains unconverted.
    fixture.play(2_048);
    assert!(fixture.owner.sources.pcm_progress(token()).is_none());
    fixture
        .owner
        .replace(Vec::new(), host_time(), host_time())
        .unwrap();
    fixture.native.borrow_mut().start();
    fixture
        .owner
        .replace(fixture.native.borrow().queues(), host_time(), host_time())
        .unwrap();
    // Both membership updates reach the same pump control slot before consumption.
    let update = allocation_counter::measure(|| fixture.update());
    assert_eq!(update.count_total, 0);
    fixture.activate();
    fixture.play(256);
    fixture.verify();
    assert_eq!(fixture.native.borrow().allocations, 1);
}

#[test]
fn coalesced_inventory_membership_prepares_pending_grants_without_old_pcm() {
    use super::super::inventory::{Job, prepare_for_test};
    let fixture = Fixture::new(48_000, 2);
    fixture.prepare();
    fixture.native.borrow_mut().start();
    fixture
        .owner
        .replace_ready(fixture.native.borrow().queues(), host_time(), host_time())
        .unwrap();
    fixture.update();
    assert!(fixture.owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Routed { head: true }
    ));
    fixture.play(2_048);
    let observed = host_time();
    let mut job = Job {
        geometry: vec![shape(2)],
        pending: fixture
            .owner
            .replace_ready(Vec::new(), observed, observed)
            .unwrap(),
        observed,
        received: observed,
    };
    assert!(job.pending.queues.is_empty());
    // Changed geometry defers the next grant for this same endpoint to inventory.
    *fixture.native.borrow_mut() = Native::new(48_000, 4);
    fixture.native.borrow_mut().start();
    let observed = host_time();
    job = Job {
        geometry: vec![shape(4)],
        pending: fixture
            .owner
            .replace_ready(fixture.native.borrow().queues(), observed, observed)
            .unwrap(),
        observed,
        received: observed,
    };
    assert_eq!(job.pending.queues.len(), 1);
    // Inventory coalescing prepares only the newest job, without a pump update.
    on_storage(|| Ok(()));
    prepare_for_test(&fixture.owner, &mut job).unwrap();
    assert!(job.pending.queues.is_empty());
    fixture.activate();
    fixture.play(256);
    fixture.verify();
}

#[test]
fn removed_endpoint_releases_its_source_and_re_admits_without_old_pcm() {
    let fixture = Fixture::new(48_000, 2);
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    fixture.native.borrow_mut().running = false;
    fixture.owner.configure(&[], host_time()).unwrap();
    assert!(fixture.owner.endpoints.lock().is_empty());
    assert!(!fixture.owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    fixture.verify();
    assert_eq!(fixture.native.borrow().allocations, 2);
}

#[test]
fn clear_during_source_preparation_cannot_publish_a_stale_owner() {
    let fixture = Fixture::new(48_000, 2);
    let weak = Rc::downgrade(&fixture.owner);
    on_storage(move || {
        weak.upgrade().unwrap().clear();
        Ok(())
    });
    assert!(matches!(
        fixture.owner.configure(&[shape(2)], host_time()),
        Err(Error::Stale)
    ));
    assert!(fixture.owner.endpoints.lock().is_empty());
    assert!(!fixture.owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
    let native = Rc::clone(&fixture.native);
    on_storage(move || {
        native.borrow_mut().slow_storage();
        Ok(())
    });
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    fixture.verify();
}

#[test]
fn geometry_change_prepares_new_storage_and_ignores_an_older_inventory() {
    let fixture = Fixture::new(192_000, 2);
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    let stale = host_time();
    let old_queues = fixture.native.borrow().queues();
    *fixture.native.borrow_mut() = Native::new(192_000, 4);
    fixture.owner.configure(&[shape(4)], host_time()).unwrap();
    fixture.owner.configure(&[shape(2)], stale).unwrap();
    fixture
        .owner
        .replace(old_queues, stale, host_time())
        .unwrap();
    assert_eq!(fixture.owner.endpoints.lock()[&[1; 16]].geometry, shape(4));
    fixture.admit();
    fixture.play(1_024);
    fixture.verify();
    assert_eq!(fixture.native.borrow().allocations, 1);
}

#[test]
fn allocation_failure_leaves_no_owner_and_the_next_inventory_can_retry() {
    let fixture = Fixture::new(48_000, 2);
    let attempts = Rc::new(Cell::new(0));
    let observed = Rc::clone(&attempts);
    on_storage(move || {
        observed.set(observed.get() + 1);
        if observed.get() == 1 {
            Err(std::io::ErrorKind::OutOfMemory.into())
        } else {
            Ok(())
        }
    });
    let result = fixture.owner.configure(&[shape(2)], host_time());
    assert!(result.unwrap_err().retryable());
    assert!(fixture.owner.endpoints.lock().is_empty());
    assert!(!fixture.owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    fixture.verify();
    assert_eq!(attempts.get(), 2);
}

#[test]
fn worker_start_failure_reclaims_the_prepared_source_and_backlog_before_retry() {
    let fixture = Fixture::new(48_000, 2);
    fail_worker();
    assert!(
        fixture
            .owner
            .configure(&[shape(2)], host_time())
            .unwrap_err()
            .retryable()
    );
    assert!(fixture.owner.endpoints.lock().is_empty());
    assert!(!fixture.owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
    fixture.prepare();
    fixture.admit();
    fixture.play(1_024);
    fixture.verify();
    assert_eq!(fixture.native.borrow().allocations, 2);
}

#[test]
fn clear_wakes_and_joins_a_prepared_worker_with_no_client_queues() {
    let owner = VirtualPcm::<ThreadWorker>::with_sources(SourceRegistry::with_clock_and_storage(
        Box::new(FixedClock),
        mixrelay_realtime::Timebase::current(),
        test_storage::create,
    ));
    owner.configure(&[shape(2)], host_time()).unwrap();
    owner.clear();
    assert!(owner.endpoints.lock().is_empty());
    assert!(!owner.sources.set_consumer_route(
        token(),
        mixrelay_macos_transport::application_clock::ConsumerRoute::Unrouted
    ));
}
struct FixedClock;
impl mixrelay_time::MonotonicClock for FixedClock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(std::time::Duration::ZERO)
    }
}

#[test]
fn current_geometry_grants_received_before_inventory_preparation_still_start_the_pump() {
    let fixture = Fixture::new(192_000, 2);
    let snapshot_at = host_time();
    fixture.owner.configure(&[shape(2)], host_time()).unwrap();
    fixture.admit_at(snapshot_at);
    fixture.play(1_024);
    fixture.verify();
}

#[test]
fn prepared_queue_handoff_does_not_wait_for_another_inventory_preparation() {
    let fixture = Fixture::new(192_000, 2);
    fixture.prepare();
    fixture.admit();
    let _preparing = fixture.owner.preparing.lock();
    fixture
        .owner
        .replace_ready(fixture.native.borrow().queues(), host_time(), host_time())
        .unwrap();
    fixture.update();
    fixture.play(1_024);
    fixture.verify();
}

#[test]
fn unprepared_first_grant_prepares_and_admits_through_the_inventory_fallback() {
    let fixture = Fixture::new(192_000, 2);
    // No publication arrived before IO started. Keep the grant through a failed
    // reservation and retry it without requiring another HAL message.
    on_storage(|| Err(std::io::ErrorKind::OutOfMemory.into()));
    fixture.native.borrow_mut().start();
    fixture.native.borrow_mut().write();
    let observed = host_time();
    let mut pending = fixture
        .owner
        .replace_ready(fixture.native.borrow().queues(), observed, observed)
        .unwrap();
    assert!(fixture.owner.endpoints.lock().is_empty());
    assert!(
        fixture
            .owner
            .configure_pending(&[shape(2)], &mut pending, observed, observed)
            .unwrap_err()
            .retryable()
    );
    assert_eq!(pending.queues.len(), 1);
    on_storage(|| Ok(()));
    fixture
        .owner
        .configure_pending(&[shape(2)], &mut pending, observed, observed)
        .unwrap();
    fixture
        .owner
        .replace_pending(&mut pending, observed, observed)
        .unwrap();
    assert!(pending.queues.is_empty());
    fixture.activate();
    fixture.play(1_024);
    fixture.verify();
}

#[test]
fn newly_prepared_endpoint_drains_while_the_next_endpoint_storage_is_blocked() {
    use super::super::inventory::{Job, prepare_for_test};
    let fixture = Rc::new(Fixture::new(192_000, 2));
    let mut other = Native::new(192_000, 4);
    other.writer = mixrelay_macos_transport::client_pcm::ClientPcmWriter::new(
        mixrelay_macos_transport::endpoint_transport::EndpointTransportConfig {
            endpoint_id: [2; 16],
            capacity_frames: QUEUE_FRAMES,
            render_channels: 4,
            capture_channels: 4,
        },
        43,
    )
    .unwrap();
    fixture.native.borrow_mut().start();
    fixture.native.borrow_mut().write();
    other.start();
    other.write();
    let observed = host_time();
    let mut queues = fixture.native.borrow().queues();
    queues.extend(other.queues());
    let mut job = Job {
        geometry: vec![
            shape(2),
            EndpointGeometry {
                id: [2; 16],
                ..shape(4)
            },
        ],
        pending: fixture
            .owner
            .replace_ready(queues, observed, observed)
            .unwrap(),
        observed,
        received: observed,
    };
    let attempts = Rc::new(Cell::new(0));
    let count = Rc::clone(&attempts);
    let weak = Rc::downgrade(&fixture);
    on_storage(move || {
        count.set(count.get() + 1);
        if count.get() == 2 {
            // B's storage call stays blocked while A runs past HAL's retention window.
            let fixture = weak.upgrade().unwrap();
            fixture.activate();
            fixture.play(QUEUE_FRAMES * 2);
            fixture.verify();
        }
        Ok(())
    });
    prepare_for_test(&fixture.owner, &mut job).unwrap();
    assert_eq!(attempts.get(), 2);
    assert!(job.pending.queues.is_empty());
}

#[test]
fn failed_endpoint_preparation_does_not_gate_other_grants_and_retries() {
    use super::super::inventory::{Job, prepare_for_test};
    let fixture = Fixture::new(48_000, 2);
    let mut other = Native::new(48_000, 4);
    other.writer = mixrelay_macos_transport::client_pcm::ClientPcmWriter::new(
        mixrelay_macos_transport::endpoint_transport::EndpointTransportConfig {
            endpoint_id: [2; 16],
            capacity_frames: QUEUE_FRAMES,
            render_channels: 4,
            capture_channels: 4,
        },
        43,
    )
    .unwrap();
    fixture.native.borrow_mut().start();
    fixture.native.borrow_mut().write();
    let observed = host_time();
    let mut queues = fixture.native.borrow().queues();
    queues.extend(other.queues());
    let mut job = Job {
        geometry: vec![
            shape(2),
            EndpointGeometry {
                id: [2; 16],
                ..shape(4)
            },
        ],
        pending: fixture
            .owner
            .replace_ready(queues, observed, observed)
            .unwrap(),
        observed,
        received: observed,
    };
    let attempts = Rc::new(Cell::new(0));
    let count = Rc::clone(&attempts);
    on_storage(move || {
        count.set(count.get() + 1);
        if count.get() == 1 {
            Ok(())
        } else {
            Err(std::io::ErrorKind::OutOfMemory.into())
        }
    });
    for attempt in 0..3 {
        assert!(
            prepare_for_test(&fixture.owner, &mut job)
                .unwrap_err()
                .retryable()
        );
        assert_eq!(
            job.pending.queues.len(),
            1,
            "only B should await preparation"
        );
        assert_eq!(job.pending.queues[0].endpoint(), [2; 16]);
        if attempt == 0 {
            fixture.activate();
        }
        fixture.play(1_024);
        assert_eq!(
            fixture
                .owner
                .sources
                .pcm_progress(token())
                .unwrap()
                .published_frames,
            fixture.native.borrow().frames
        );
    }
    fixture.verify();
    assert_eq!(attempts.get(), 4, "A must not be allocated again");
    on_storage(|| Ok(()));
    prepare_for_test(&fixture.owner, &mut job).unwrap();
    assert!(job.pending.queues.is_empty());
    let endpoints = fixture.owner.endpoints.lock();
    let admitted = endpoints[&[2; 16]].update.lock().take().unwrap();
    assert_eq!(admitted.queues.len(), 1);
    assert_eq!(admitted.queues[0].endpoint(), [2; 16]);
}
