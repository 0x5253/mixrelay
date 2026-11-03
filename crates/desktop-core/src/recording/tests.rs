use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Debug, Default)]
struct Repository {
    bytes: Mutex<Option<Vec<u8>>>,
    fail: AtomicBool,
}
impl PreferencesRepository for Repository {
    fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.bytes.lock().unwrap().clone())
    }
    fn replace(&self, bytes: &[u8]) -> std::io::Result<()> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("commit failed"));
        }
        *self.bytes.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}
#[derive(Debug)]
struct Folders;
impl RecordingDirectories for Folders {
    fn exists(&self, path: &Path) -> bool {
        path == Path::new("/recordings")
    }
}
struct Names(AtomicU64);
impl TakeIdentities for Names {
    fn next(&self) -> Result<TakeIdentity, RecordingError> {
        Ok(TakeIdentity {
            nanos: 42,
            owner: 7,
            sequence: self.0.fetch_add(1, Ordering::Relaxed),
        })
    }
}
fn store(repository: Arc<Repository>) -> RecordingStore {
    RecordingStore::load(repository, Arc::new(Folders), PathBuf::from("/recordings")).unwrap()
}

#[test]
fn commit_failure_preserves_the_previous_preferences_and_restart_state() {
    let repository = Arc::new(Repository::default());
    let mut current = store(Arc::clone(&repository));
    let mut settings = current.get("g", "n");
    settings.sample_rate = Some(96_000);
    settings.encoding = RecordingEncoding::Pcm24;
    current
        .save("g".into(), "n".into(), settings.clone())
        .unwrap();
    repository.fail.store(true, Ordering::Relaxed);
    let mut rejected = settings.clone();
    rejected.encoding = RecordingEncoding::Float32;
    assert!(current.save("g".into(), "n".into(), rejected).is_err());
    assert_eq!(current.get("g", "n"), settings);
    assert_eq!(store(repository).get("g", "n"), settings);
    assert_eq!(current.get("other", "n").sample_rate, None);
}

#[test]
fn rejected_rate_or_missing_folder_never_replaces_the_document() {
    let repository = Arc::new(Repository::default());
    let mut current = store(Arc::clone(&repository));
    let mut settings = current.get("g", "n");
    settings.sample_rate = Some(123);
    assert!(matches!(
        current.save("g".into(), "n".into(), settings.clone()),
        Err(RecordingError::SampleRate)
    ));
    settings.sample_rate = None;
    settings.folder = "/absent".into();
    assert!(matches!(
        current.save("g".into(), "n".into(), settings),
        Err(RecordingError::MissingFolder)
    ));
    assert!(repository.bytes.lock().unwrap().is_none());
}

#[test]
fn deterministic_names_are_unique_and_resume_never_reserves_an_identity() {
    let current = store(Arc::new(Repository::default()));
    let settings = current.get("g", "n");
    let names = Names(AtomicU64::new(0));
    let action = serde_json::json!({"action":"record"});
    let first = prepare_transport(
        &action,
        &serde_json::json!({"state":"stopped"}),
        &settings,
        &names,
    )
    .unwrap();
    assert_eq!(first["path"], "/recordings/MixRelay-42-7-0.wav");
    let resumed = prepare_transport(
        &action,
        &serde_json::json!({"state":"paused"}),
        &settings,
        &names,
    )
    .unwrap();
    assert_eq!(resumed, action);
    let second = prepare_transport(
        &action,
        &serde_json::json!({"state":"failed"}),
        &settings,
        &names,
    )
    .unwrap();
    assert_eq!(second["path"], "/recordings/MixRelay-42-7-1.wav");
    assert_eq!(first["encoding"], "pcm16");
}

#[test]
fn only_the_observed_stopped_recorder_can_change_preferences() {
    let mut snapshot = serde_json::json!({"revision":9,"graphs":[{"id":"g","nodes":[{"nodeId":"n","capabilities":{"recorder":true},"recorder":{"state":"stopped"}}]}]});
    assert!(validate_target(&snapshot, "g", "n", 9).is_ok());
    assert!(matches!(
        validate_target(&snapshot, "g", "n", 8),
        Err(RecordingError::RevisionChanged)
    ));
    assert!(matches!(
        validate_target(&snapshot, "g", "removed", 9),
        Err(RecordingError::MissingRecorder)
    ));
    for state in ["recording", "paused"] {
        snapshot["graphs"][0]["nodes"][0]["recorder"]["state"] = state.into();
        assert!(matches!(
            validate_target(&snapshot, "g", "n", 9),
            Err(RecordingError::RecorderActive)
        ));
    }
}
