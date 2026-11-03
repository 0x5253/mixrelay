use super::*;
use crate::{AudioLayout, WORKER_READY, shared_transport};

#[test]
fn native_capture_requests_authoritative_state_and_returns_the_workers_payload() {
    let (_, mailbox, _, worker) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    worker
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let control = HostedProcessorControl::native(mailbox, Arc::new(mixrelay_time::StdClock::new()));
    let responder = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let (request, kind) = loop {
            if let Some(pending) = worker.pending_request() {
                break pending;
            }
            assert!(
                Instant::now() < deadline,
                "capture request was not published"
            );
            thread::yield_now();
        };
        assert_eq!(kind, editor_surface::NATIVE_STATE);
        worker.publish(request, Ok(vec![1, 2, 3]));
    });
    assert_eq!(
        control.snapshot_state(Duration::from_secs(5)),
        Ok(vec![1, 2, 3])
    );
    responder.join().unwrap();
}
