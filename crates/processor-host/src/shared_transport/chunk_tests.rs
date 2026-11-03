use std::{
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

use mixrelay_test_support::FixtureDir;

use super::*;
use crate::{ProcessorStateError, STATE_CAPACITY, STATE_FAILED, WORKER_READY};

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "control handshake did not complete"
        );
        thread::yield_now();
    }
}

fn layout() -> crate::AudioLayout {
    crate::AudioLayout {
        inputs: 2,
        outputs: 2,
        block_frames: 128,
    }
}

#[test]
fn empty_exact_and_partial_chunks_preserve_payloads_across_requests() {
    let directory = FixtureDir::new("processor-chunk-boundaries").unwrap();
    let path = directory.join("audio");
    let (_audio, parent) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let sizes = [
        0,
        1,
        CHUNK_CAPACITY - 1,
        CHUNK_CAPACITY,
        CHUNK_CAPACITY + 1,
        STATE_CAPACITY,
    ];
    let child = thread::spawn(move || {
        for size in sizes {
            wait_until(|| worker.pending_request().is_some());
            let (request, _) = worker.pending_request().unwrap();
            worker.publish(request, Ok(vec![0x5a; size]));
        }
    });
    for size in sizes {
        assert_eq!(
            parent
                .try_lock()
                .unwrap()
                .request_blob(0, Duration::from_secs(2))
                .unwrap(),
            vec![0x5a; size]
        );
    }
    child.join().unwrap();
}

#[test]
fn cancelled_unconsumed_chunk_is_acknowledged_before_reuse() {
    let directory = FixtureDir::new("processor-chunk-cancel").unwrap();
    let path = directory.join("audio");
    let (_audio, parent) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    parent.header().state_request.store(1, Ordering::Release);
    let child = thread::spawn(move || {
        worker.publish(1, Ok(vec![7; STATE_CAPACITY]));
        wait_until(|| worker.pending_request().is_some());
        let (request, _) = worker.pending_request().unwrap();
        assert_eq!(request, 2);
        worker.publish(request, Ok(vec![9; CHUNK_CAPACITY + 1]));
    });
    wait_until(|| parent.header().state_written.load(Ordering::Acquire) != 0);
    // Deliberately do not acknowledge the chunk. Cancellation must release the broker.
    parent.header().state_cancel.store(1, Ordering::Release);
    wait_until(|| parent.header().state_response.load(Ordering::Acquire) == 1);
    assert_eq!(
        parent
            .try_lock()
            .unwrap()
            .request_blob(0, Duration::from_secs(2))
            .unwrap(),
        vec![9; CHUNK_CAPACITY + 1]
    );
    child.join().unwrap();
}

#[test]
fn capture_completed_after_timeout_cannot_publish_payload() {
    let directory = FixtureDir::new("processor-late-capture").unwrap();
    let path = directory.join("audio");
    let (_audio, parent) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    assert_eq!(
        parent.try_lock().unwrap().request_blob(0, Duration::ZERO),
        Err(ProcessorStateError::Timeout)
    );
    assert_eq!(
        parent.try_lock().unwrap().request_blob(0, Duration::ZERO),
        Err(ProcessorStateError::Pending)
    );
    worker.publish(1, Ok(vec![7; STATE_CAPACITY]));
    assert_eq!(parent.header().state_written.load(Ordering::Acquire), 0);
    assert_eq!(parent.header().state_response.load(Ordering::Acquire), 1);
}

#[test]
fn generation_wrap_skips_the_no_cancellation_sentinel() {
    let directory = FixtureDir::new("processor-generation-wrap").unwrap();
    let path = directory.join("audio");
    let (_audio, parent) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    parent
        .header()
        .state_request
        .store(u64::MAX, Ordering::Relaxed);
    parent
        .header()
        .state_cancel
        .store(u64::MAX, Ordering::Relaxed);
    parent
        .header()
        .state_response
        .store(u64::MAX, Ordering::Release);
    let child = thread::spawn(move || {
        wait_until(|| worker.pending_request().is_some());
        let (request, _) = worker.pending_request().unwrap();
        assert_eq!(request, 1);
        worker.publish(request, Ok(vec![42]));
    });
    assert_eq!(
        parent
            .try_lock()
            .unwrap()
            .request_blob(0, Duration::from_secs(2))
            .unwrap(),
        vec![42]
    );
    child.join().unwrap();
}

#[test]
fn invalid_chunk_progress_cancels_without_borrowing_out_of_bounds() {
    let directory = FixtureDir::new("processor-invalid-chunk").unwrap();
    let path = directory.join("audio");
    let (_audio, parent) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let child = thread::spawn(move || {
        wait_until(|| worker.pending_request().is_some());
        worker.header().state_written.store(
            u32::try_from(CHUNK_CAPACITY + 1).unwrap(),
            Ordering::Release,
        );
        wait_until(|| worker.header().state_cancel.load(Ordering::Acquire) == 1);
    });
    assert_eq!(
        parent
            .try_lock()
            .unwrap()
            .request_blob(0, Duration::from_secs(2)),
        Err(ProcessorStateError::InvalidResponse)
    );
    child.join().unwrap();
}

#[test]
fn failed_partial_response_reports_failure_without_returning_partial_bytes() {
    let directory = FixtureDir::new("processor-partial-response-failure").unwrap();
    let path = directory.join("audio");
    let (_audio, parent) = create(
        &path,
        layout(),
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let child = thread::spawn(move || {
        wait_until(|| worker.pending_request().is_some());
        // The fresh mapping's bytes are zero; publish a partial chunk then terminal failure.
        worker.header().state_written.store(4, Ordering::Release);
        worker.header().state_length.store(0, Ordering::Relaxed);
        worker
            .header()
            .state_result
            .store(STATE_FAILED, Ordering::Relaxed);
        worker.header().state_response.store(1, Ordering::Release);
    });
    assert_eq!(
        parent
            .try_lock()
            .unwrap()
            .request_blob(0, Duration::from_secs(2)),
        Err(ProcessorStateError::NativeFailure)
    );
    child.join().unwrap();
}
