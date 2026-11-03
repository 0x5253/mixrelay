use super::*;

#[test]
fn command_and_shutdown_preserve_admission_and_finalization_failures() {
    use crate::{AdmissionFailure, RecorderFailure, recording_files::faults::FaultFiles};
    use std::sync::atomic::{AtomicU8, Ordering};

    for shutdown in [false, true] {
        let files = MemoryFiles::default();
        let path = Path::new("/rec-failure/take.wav");
        let fault = Arc::new(AtomicU8::new(0));
        let transport = Transport::new(1, std::hint::spin_loop).unwrap();
        let tap = RecorderTap::new(Arc::clone(&transport));
        let status = Arc::new(Mutex::new(RecorderStatus::default()));
        let mut worker = Worker::new(
            transport,
            Arc::clone(&status),
            FaultFiles(files.clone(), Arc::clone(&fault)),
        )
        .unwrap();
        tap.capture(&[], 48_000);
        worker
            .record(&RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().into()),
                sample_rate: None,
                encoding: Some(Encoding::Float32),
            })
            .unwrap();
        tap.capture(&[0.25; 2], 48_000);
        tap.capture(&vec![0.5; 4097], 48_000);
        fault.store(4, Ordering::Relaxed);
        let error = if shutdown {
            worker.shutdown().unwrap_err()
        } else {
            worker
                .command(&RecorderRequest {
                    action: RecorderAction::Stop,
                    path: None,
                    sample_rate: None,
                    encoding: None,
                })
                .unwrap_err()
        };
        let RecorderFailure::Combined { primary, secondary } = error.failure() else {
            panic!("admission and finalization must both survive: {error:?}");
        };
        assert!(matches!(
            primary.failure(),
            RecorderFailure::Admission(AdmissionFailure::InvalidPcm)
        ));
        assert!(
            matches!(secondary.failure(), RecorderFailure::Finalize { path: failed, source }
            if failed == path && source.kind() == std::io::ErrorKind::Other)
        );
        let state = status.lock().clone();
        assert_eq!((state.state, state.frames), (RecorderState::Failed, 2));
        assert_eq!(state.error.as_deref(), Some(error.to_string().as_str()));
        assert!(files.bytes(path).ends_with(&[0, 0, 128, 62, 0, 0, 128, 62]));
        assert!(worker.shutdown().unwrap_err().same_failure(&error));
    }
}

#[test]
fn sticky_write_is_reported_once_while_distinct_extent_failure_is_retained() {
    use crate::{RecorderFailure, recording_files::faults::FaultFiles};
    use std::sync::atomic::{AtomicU8, Ordering};

    for finalization_fault in [0, 4] {
        let files = MemoryFiles::default();
        let path = Path::new("/rec-write/take.wav");
        let fault = Arc::new(AtomicU8::new(0));
        let transport = Transport::new(1, std::hint::spin_loop).unwrap();
        let tap = RecorderTap::new(Arc::clone(&transport));
        let status = Arc::new(Mutex::new(RecorderStatus::default()));
        let mut worker =
            Worker::new(transport, status, FaultFiles(files, Arc::clone(&fault))).unwrap();
        tap.capture(&[], 48_000);
        worker
            .record(&RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().into()),
                sample_rate: None,
                encoding: Some(Encoding::Float32),
            })
            .unwrap();
        fault.store(1, Ordering::Relaxed);
        tap.capture(&[0.25; PACKET_FRAMES], 48_000);
        let write = worker.drain().unwrap_err();
        assert!(matches!(write.failure(), RecorderFailure::Write { .. }));
        fault.store(finalization_fault, Ordering::Relaxed);
        let failure = worker.fail(write.clone());
        if finalization_fault == 0 {
            assert!(failure.same_failure(&write));
        } else {
            let RecorderFailure::Combined { primary, secondary } = failure.failure() else {
                panic!("only distinct causes are aggregated: {failure:?}");
            };
            assert!(primary.same_failure(&write));
            assert!(matches!(
                secondary.failure(),
                RecorderFailure::Finalize { .. }
            ));
        }
        assert!(worker.shutdown().unwrap_err().same_failure(&failure));
    }
}

fn causes(error: &crate::RecorderError) -> Vec<&crate::RecorderFailure> {
    match error.failure() {
        crate::RecorderFailure::Combined { primary, secondary } => {
            let mut result = causes(primary);
            result.extend(causes(secondary));
            result
        }
        failure => vec![failure],
    }
}

#[test]
fn admission_and_write_failures_keep_each_distinct_source_once() {
    use crate::{AdmissionFailure, RecorderFailure, recording_files::faults::FaultFiles};
    use std::sync::atomic::{AtomicU8, Ordering};

    for mode in [6, 5] {
        let files = MemoryFiles::default();
        let path = Path::new("/rec-admission-write/take.wav");
        let fault = Arc::new(AtomicU8::new(0));
        let transport = Transport::new(1, std::hint::spin_loop).unwrap();
        let tap = RecorderTap::new(Arc::clone(&transport));
        let status = Arc::new(Mutex::new(RecorderStatus::default()));
        let mut worker = Worker::new(
            transport,
            Arc::clone(&status),
            FaultFiles(files, Arc::clone(&fault)),
        )
        .unwrap();
        tap.capture(&[], 48_000);
        worker
            .record(&RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().into()),
                sample_rate: None,
                encoding: Some(Encoding::Float32),
            })
            .unwrap();
        fault.store(mode, Ordering::Relaxed);
        tap.capture(&[0.25; 2], 48_000);
        tap.capture(&vec![0.5; 4097], 48_000);
        let error = worker.shutdown().unwrap_err();
        let failures = causes(&error);
        assert_eq!(failures.len(), if mode == 6 { 2 } else { 3 });
        assert!(matches!(
            failures[0],
            RecorderFailure::Admission(AdmissionFailure::InvalidPcm)
        ));
        assert!(
            matches!(failures[1], RecorderFailure::Write { path: failed, source: hound::Error::IoError(source) }
            if failed == path && source.kind() == std::io::ErrorKind::Other)
        );
        if mode == 5 {
            assert!(
                matches!(failures[2], RecorderFailure::Finalize { path: failed, source }
                if failed == path && source.kind() == std::io::ErrorKind::Other)
            );
        }
        let snapshot = status.lock().clone();
        assert_eq!(
            (snapshot.state, snapshot.frames),
            (RecorderState::Failed, 0)
        );
        assert_eq!(snapshot.error, Some(error.to_string()));
    }
}
