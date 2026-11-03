use super::*;
use crate::{
    RecorderAction, recording_files::memory::MemoryFiles, transport::Transport, worker::Worker,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Default)]
struct DirectWorkers {
    files: MemoryFiles,
    created: Arc<AtomicUsize>,
    shutdowns: Arc<AtomicUsize>,
    reject_next: Arc<Mutex<Option<RecorderError>>>,
    fail_cleanup: Arc<Mutex<Option<RecorderError>>>,
}

struct DirectSession {
    transport: Arc<Transport>,
    status: Arc<Mutex<RecorderStatus>>,
    worker: Mutex<Worker<MemoryFiles>>,
    shutdowns: Arc<AtomicUsize>,
    fail_cleanup: Option<RecorderError>,
    stopped: bool,
}

impl SessionFactory for DirectWorkers {
    type Handle = DirectSession;
    fn prepare(&self, channels: u16) -> Result<(Self::Handle, RecorderTap), RecorderError> {
        self.created.fetch_add(1, Ordering::Relaxed);
        if let Some(failure) = self.reject_next.lock().take() {
            return Err(failure);
        }
        let transport = Transport::new(channels, std::hint::spin_loop).unwrap();
        let tap = RecorderTap::new(Arc::clone(&transport));
        let status = Arc::new(Mutex::new(RecorderStatus::default()));
        let worker = Worker::new(
            Arc::clone(&transport),
            Arc::clone(&status),
            self.files.clone(),
        )
        .unwrap();
        Ok((
            DirectSession {
                transport,
                status,
                worker: Mutex::new(worker),
                shutdowns: Arc::clone(&self.shutdowns),
                fail_cleanup: self.fail_cleanup.lock().take(),
                stopped: false,
            },
            tap,
        ))
    }
}

impl SessionHandle for DirectSession {
    fn channels(&self) -> u16 {
        self.transport.channels
    }
    fn live_taps(&self) -> usize {
        self.transport.taps.load(Ordering::SeqCst)
    }
    fn tap(&self) -> RecorderTap {
        RecorderTap::new(Arc::clone(&self.transport))
    }
    fn status(&self) -> RecorderStatus {
        self.status.lock().clone()
    }
    fn command(&self, request: RecorderRequest) -> Result<RecorderStatus, RecorderError> {
        self.worker.lock().command(&request)
    }
    fn shutdown(&mut self) -> Result<(), RecorderError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        self.shutdowns.fetch_add(1, Ordering::Relaxed);
        let result = self.worker.lock().shutdown();
        self.fail_cleanup.take().map_or(result, Err)
    }
}

#[test]
fn preparation_uses_the_shared_one_to_eight_channel_contract() {
    let registry = Sessions::new(DirectWorkers::default());
    for channels in 1..=mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS {
        assert!(
            registry
                .prepare(&format!("channels-{channels}"), channels)
                .is_ok()
        );
    }
    for channels in [
        0,
        mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS + 1,
        64,
        u16::MAX,
    ] {
        assert!(registry.prepare("unsupported", channels).is_err());
    }
}

#[test]
fn sixteen_live_sessions_reject_an_extra_and_reclaim_only_a_retired_slot() {
    let registry = Sessions::new(DirectWorkers::default());
    let taps: Vec<_> = (0..15)
        .map(|index| registry.prepare(&format!("rec-{index}"), 1).unwrap())
        .collect();
    let last = registry.prepare("last", 8).unwrap();
    assert!(
        registry
            .prepare("extra", 1)
            .unwrap_err()
            .to_string()
            .contains("capacity")
    );
    drop(last);
    let replacement = registry.prepare("extra", 2).unwrap();
    for tap in &taps {
        tap.capture(&[0.25], 48_000);
    }
    replacement.capture(&[0.5; 2], 48_000);
    // The sixteen retained identities still occupy the pool after reclamation.
    assert!(
        registry
            .prepare("over", 1)
            .unwrap_err()
            .to_string()
            .contains("capacity")
    );
}

#[test]
fn retired_session_churn_does_not_require_a_status_client() {
    let registry = Sessions::new(DirectWorkers::default());
    let retained = registry.prepare("retained", 1).unwrap();
    for index in 0..40 {
        drop(registry.prepare(&format!("retired-{index}"), 1).unwrap());
    }
    retained.capture(&[], 48_000);
    registry.reap_unused().unwrap();
    assert!(registry.prepare("retained", 2).is_err());
}

#[test]
fn validation_and_reuse_do_not_prepare_a_worker_and_spawn_failure_can_retry() {
    let workers = DirectWorkers::default();
    let registry = Sessions::new(workers.clone());
    assert!(matches!(
        registry.prepare(" ", 1).unwrap_err().failure(),
        RecorderFailure::InvalidIdentity
    ));
    assert_eq!(workers.created.load(Ordering::Relaxed), 0);
    let original = registry.prepare("rec", 1).unwrap();
    let reused = registry.prepare("rec", 1).unwrap();
    assert!(Arc::ptr_eq(&original.shared, &reused.shared));
    assert!(matches!(
        registry.prepare("rec", 2).unwrap_err().failure(),
        RecorderFailure::ActiveChannels
    ));
    assert_eq!(workers.created.load(Ordering::Relaxed), 1);
    drop((original, reused));
    *workers.reject_next.lock() = Some(
        RecorderFailure::WorkerSpawn(std::io::Error::from(std::io::ErrorKind::WouldBlock)).into(),
    );
    let failure = registry.prepare("rec", 2).unwrap_err();
    assert!(
        matches!(failure.failure(), RecorderFailure::WorkerSpawn(source) if source.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert_eq!(workers.shutdowns.load(Ordering::Relaxed), 1);
    assert!(registry.handles.lock().is_empty());
    let replacement = registry.prepare("rec", 2).unwrap();
    assert_eq!(replacement.shared.channels, 2);
    drop(registry);
    assert_eq!(workers.shutdowns.load(Ordering::Relaxed), 2);
}

#[test]
fn reaping_continues_after_failures_and_reports_every_retired_identity_once() {
    let workers = DirectWorkers::default();
    let registry = Sessions::new(workers.clone());
    *workers.fail_cleanup.lock() = Some(RecorderFailure::WorkerPanicked.into());
    drop(registry.prepare("first", 1).unwrap());
    *workers.fail_cleanup.lock() = Some(RecorderFailure::WorkerUnacknowledged.into());
    drop(registry.prepare("second", 1).unwrap());
    drop(registry.prepare("third", 1).unwrap());
    let live = registry.prepare("live", 1).unwrap();
    let error = registry.reap_unused().unwrap_err();
    let RecorderFailure::Combined { primary, secondary } = error.failure() else {
        panic!("both retired failures must survive: {error:?}");
    };
    assert!(
        matches!(primary.failure(), RecorderFailure::Session { id, path: None, source }
        if id == "first" && matches!(source.failure(), RecorderFailure::WorkerPanicked))
    );
    assert!(
        matches!(secondary.failure(), RecorderFailure::Session { id, path: None, source }
        if id == "second" && matches!(source.failure(), RecorderFailure::WorkerUnacknowledged))
    );
    assert_eq!(workers.shutdowns.load(Ordering::Relaxed), 3);
    assert_eq!(registry.handles.lock().len(), 1);
    assert!(registry.reap_unused().is_ok());
    drop(live);
    registry.reap_unused().unwrap();
    assert_eq!(workers.shutdowns.load(Ordering::Relaxed), 4);
}

#[test]
fn commands_keep_open_take_taps_and_stop_all_processes_good_and_failed_sessions() {
    let workers = DirectWorkers::default();
    let registry = Sessions::new(workers.clone());
    let good = registry.prepare("good", 1).unwrap();
    let bad = registry.prepare("bad", 1).unwrap();
    for (id, tap) in [("good", &good), ("bad", &bad)] {
        tap.capture(&[], 48_000);
        registry
            .command(
                id,
                RecorderRequest {
                    action: RecorderAction::Record,
                    path: Some(format!("/rec-session/{id}.wav")),
                    sample_rate: None,
                    encoding: Some(crate::Encoding::Float32),
                },
            )
            .unwrap();
    }
    let held = registry.open_take_taps();
    assert_eq!(held.len(), 2);
    assert!(
        held.iter()
            .all(|(_, _, state)| *state == RecorderState::Recording)
    );
    good.capture(&[0.25; 2], 48_000);
    bad.capture(&vec![0.5; 4097], 48_000);
    let failure = registry.stop_all().unwrap_err();
    let RecorderFailure::Sessions(failures) = failure.failure() else {
        panic!("{failure:?}");
    };
    assert_eq!(failures.len(), 1);
    assert!(
        matches!(failures[0].failure(), RecorderFailure::Session { id, path: Some(path), source }
        if id == "bad" && path == "/rec-session/bad.wav" && matches!(source.failure(), RecorderFailure::Admission(_)))
    );
    assert_eq!(registry.status("good").frames, 2);
    assert_eq!(registry.status("good").state, RecorderState::Stopped);
    assert_eq!(
        workers
            .files
            .reader(Path::new("/rec-session/good.wav"))
            .duration(),
        2
    );
    assert!(registry.open_take_taps().is_empty());
    assert!(matches!(
        registry
            .command(
                "missing",
                RecorderRequest {
                    action: RecorderAction::Stop,
                    path: None,
                    sample_rate: None,
                    encoding: None
                }
            )
            .unwrap_err()
            .failure(),
        RecorderFailure::Unprepared
    ));
}
