//! Worker terminal errors wake control and retire offline recovery owners.
use super::*;
use mixrelay_audio_io::recovery::RecoveryMailbox;
use mixrelay_linux_pipewire::DriverCycle;
use std::{
    future::pending,
    sync::{Arc, Mutex},
};

fn stopped() -> Termination {
    Termination {
        startup_timed_out: false,
        disconnected: false,
        stop_requested: true,
        processing_failure: None,
        native_failure: None,
    }
}

#[test]
fn requested_stop_and_local_resource_rejection_do_not_fail_the_worker() {
    let resource_failures = FailureMailbox::default();
    resource_failures.publish_native(Failure::RetirementPending, -32);
    report_resource_failure(&resource_failures);
    assert_eq!(resource_failures.read(), None);
    assert_eq!(stopped().result(), Ok(()));
    assert!(
        Termination {
            stop_requested: false,
            disconnected: true,
            ..stopped()
        }
        .result()
        .unwrap_err()
        .disconnected
    );
}

#[test]
fn stop_request_preserves_every_terminal_failure_after_native_drain() {
    let result = Termination {
        startup_timed_out: true,
        processing_failure: Some(Failure::TimestampRegressed),
        native_failure: Some(DriverProcessFailure::DependencyQueue(-22)),
        ..stopped()
    }
    .result()
    .unwrap_err();
    assert!(result.startup_timed_out);
    assert_eq!(result.processing_failure, Some(Failure::TimestampRegressed));
    assert_eq!(
        result.native_failure,
        Some(DriverProcessFailure::DependencyQueue(-22))
    );
}

struct PendingOwner(Arc<Mutex<Vec<thread::ThreadId>>>);

impl Drop for PendingOwner {
    fn drop(&mut self) {
        self.0.lock().unwrap().push(thread::current().id());
    }
}

#[tokio::test]
async fn idle_worker_fault_closes_control_and_retires_pending_recovery_without_a_cycle() {
    let drops = Arc::new(Mutex::new(Vec::new()));
    let mailbox = Arc::new(RecoveryMailbox::default());
    assert!(mailbox.publish(2, PendingOwner(Arc::clone(&drops))).is_ok());
    let worker_mailbox = Arc::clone(&mailbox);
    let status = Arc::new(AdapterRuntimeStatus::new(
        crate::runtime::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    let worker_status = Arc::clone(&status);
    let (sender, mut completion) = mixrelay_engine_core::worker_completion::channel();
    let worker = thread::spawn(move || {
        let result = {
            let _lease = worker_status.begin_processing().unwrap();
            let _consumer = worker_mailbox.try_consumer().unwrap();
            // No callback activates the prepared replacement after this clock fault.
            Termination {
                stop_requested: false,
                native_failure: Some(DriverProcessFailure::InvalidPosition),
                ..stopped()
            }
            .result()
        };
        sender.send(result).unwrap();
    });
    let error = tokio::time::timeout(Duration::from_secs(1), completion.shutdown(pending()))
        .await
        .expect("worker failure waited for an audio callback")
        .unwrap_err();
    assert!(error.to_string().contains("InvalidPosition"));
    worker.join().unwrap();
    assert!(
        completion
            .finish()
            .await
            .unwrap_err()
            .to_string()
            .contains("InvalidPosition")
    );
    assert_eq!(
        mixrelay_engine_core::RuntimeStatusBackend::status(
            &*status,
            crate::runtime::render_format(),
        )
        .audio_session
        .unwrap()
        .issue,
        Some(mixrelay_audio_io::session::SessionIssue::ProcessingFailed)
    );
    assert!(status.begin_processing().is_err());
    assert!(mailbox.try_consumer().is_some());
    assert!(!mailbox.idle());
    assert!(drops.lock().unwrap().is_empty());
    drop(mailbox);
    assert_eq!(*drops.lock().unwrap(), vec![thread::current().id()]);
}

#[test]
fn control_poll_reports_each_mailbox_event_once() {
    let mailbox = clock::DiscontinuityMailbox::default();
    let previous = DriverCycle {
        position: 0,
        monotonic_ns: 1,
        clock_id: 2,
        rate: 48_000,
        frames: 128,
    };
    let event = clock::NativeDiscontinuity {
        previous,
        current: DriverCycle {
            position: 256,
            monotonic_ns: 2,
            ..previous
        },
        mapped_epoch: 2,
        render_position: 128,
    };
    let mut last_event = 0;
    report_clock_discontinuity(&mailbox, &mut last_event);
    assert_eq!(last_event, 0);
    mailbox.publish(event);
    report_clock_discontinuity(&mailbox, &mut last_event);
    assert_eq!(last_event, 1);
    report_clock_discontinuity(&mailbox, &mut last_event);
    assert_eq!(last_event, 1);
}
