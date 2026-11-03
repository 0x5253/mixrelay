use std::{
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

use mixrelay_test_support::FixtureDir;

use super::*;
use crate::{WORKER_READY, editor_surface};

#[test]
fn response_round_trip_measurement() {
    let directory = FixtureDir::new("processor-response-measurement").unwrap();
    let path = directory.join("audio");
    let layout = crate::AudioLayout {
        inputs: 2,
        outputs: 2,
        block_frames: 128,
    };
    let (_audio, parent) = create(
        &path,
        layout,
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, worker) = worker(&path).unwrap();
    parent
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    eprintln!(
        "response mapping_bytes={}",
        std::fs::metadata(&path).unwrap().len()
    );
    let (ready, prepared) = std::sync::mpsc::sync_channel(0);
    let child = thread::spawn(move || {
        for size in [256 * 1024, 1024 * 1024, 4 * 1024 * 1024] {
            // Construction precedes each request, outside the parent's timed round trip.
            for _ in 0..31 {
                let bytes = vec![0xa5; size];
                ready.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while worker.pending_request().is_none() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_micros(100));
                }
                let (request, kind) = worker.pending_request().unwrap();
                assert_eq!(kind, editor_surface::EDITOR_FRAME);
                worker.publish(request, Ok(bytes));
            }
        }
    });
    for size in [256 * 1024, 1024 * 1024, 4 * 1024 * 1024] {
        let mut elapsed = Vec::with_capacity(30);
        for sample in 0..31 {
            prepared.recv_timeout(Duration::from_secs(5)).unwrap();
            let start = Instant::now();
            let bytes = parent
                .try_lock()
                .unwrap()
                .request_blob(editor_surface::EDITOR_FRAME, Duration::from_secs(2))
                .unwrap();
            let micros = start.elapsed().as_micros();
            assert_eq!(bytes.len(), size);
            assert!(bytes.iter().all(|byte| *byte == 0xa5));
            if sample != 0 {
                elapsed.push(micros);
            }
        }
        elapsed.sort_unstable();
        eprintln!(
            "response bytes={size} samples=30 median_us={} max_us={}",
            elapsed[15], elapsed[29]
        );
    }
    child.join().unwrap();
}
