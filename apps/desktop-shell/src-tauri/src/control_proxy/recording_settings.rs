//! Destination preferences belong to the background controller, outside the audio plan.
//! UI edits and hardware starts share the same serialized store and take preparation.
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use mixrelay_desktop_core::recording::{RecordingSettings, RecordingStore};
use mixrelay_desktop_core::{
    engine_control::routes,
    recording::{self, RecordingDirectories, RecordingError, TakeIdentities, TakeIdentity},
};
use tauri::Manager as _;
use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard};

use super::{ControlProxyClient, Method};

/// Next-take preferences and the gate that serializes their commits with recorder starts.
#[derive(Debug, Default)]
pub(super) struct Recordings {
    /// Readers keep the previous snapshot while a save is written.
    store: RwLock<RecordingStore>,
    /// Held across a preference commit or a recorder submission; shutdown waits for it.
    gate: Arc<Mutex<()>>,
    take_identities: NativeTakeIdentities,
}

impl Recordings {
    pub(super) async fn lock_commits(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().await
    }

    async fn lock_commits_owned(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.gate).lock_owned().await
    }

    pub(super) fn preferences(&self, graph: &str, node: &str) -> Result<RecordingSettings, String> {
        Ok(self
            .store
            .read()
            .map_err(|_| "recording settings lock poisoned")?
            .get(graph, node))
    }

    pub(super) fn take_identities(&self) -> &impl TakeIdentities {
        &self.take_identities
    }

    fn replace(&self, store: RecordingStore) -> Result<(), String> {
        *self
            .store
            .write()
            .map_err(|_| "recording settings lock poisoned")? = store;
        Ok(())
    }

    /// Persists, then publishes. Once persistence starts, cancellation cannot separate the
    /// disk commit from publication: the owned `gate` guard moves into the blocking commit.
    fn commit(
        &self,
        gate: OwnedMutexGuard<()>,
        (graph, node): (String, String),
        settings: RecordingSettings,
    ) -> Result<(), String> {
        let _gate = gate;
        let mut store = self
            .store
            .read()
            .map_err(|_| "recording settings lock poisoned")?
            .clone();
        store
            .save(graph, node, settings)
            .map_err(|error| error.to_string())?;
        self.replace(store)
    }
}

/// Resolve user-owned defaults once during controller setup, before admitting controllers.
pub(crate) fn initialize(app: &tauri::App) -> Result<(), String> {
    let path = crate::background::data_directory(app.handle())?.join("recording-settings.json");
    let desktop = crate::recording_dialog::default_recording_folder(app)?;
    let store = load_store(path, desktop).map_err(|error| error.to_string())?;
    app.state::<ControlProxyClient>().recordings.replace(store)
}

#[derive(Debug)]
struct NativeFolders;
impl RecordingDirectories for NativeFolders {
    fn exists(&self, path: &std::path::Path) -> bool {
        path.is_dir()
    }
}
fn load_store(path: PathBuf, folder: PathBuf) -> Result<RecordingStore, RecordingError> {
    RecordingStore::load(
        Arc::new(crate::preferences_file::AtomicPreferences::new(
            path,
            recording::FILE_LIMIT,
        )),
        Arc::new(NativeFolders),
        folder,
    )
}

#[derive(Debug, Default)]
struct NativeTakeIdentities {
    sequence: std::sync::atomic::AtomicU64,
}
impl TakeIdentities for NativeTakeIdentities {
    fn next(&self) -> Result<TakeIdentity, RecordingError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(RecordingError::IdentityClock)?
            .as_nanos();
        // The counter supplies uniqueness only; it does not publish other take state.
        let sequence = self
            .sequence
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |previous| previous.checked_add(1),
            )
            .map_err(|_| RecordingError::IdentityExhausted)?;
        Ok(TakeIdentity {
            nanos,
            owner: std::process::id(),
            sequence,
        })
    }
}

/// Read the same next-take settings used by local hardware commands.
#[tauri::command]
pub(crate) async fn recording_settings(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
    node_id: String,
) -> Result<RecordingSettings, String> {
    client.recordings.preferences(&graph_id, &node_id)
}

/// Commit a stopped recorder's preferences without modifying its audio graph.
#[tauri::command]
pub(crate) async fn save_recording_settings(
    client: tauri::State<'_, ControlProxyClient>,
    graph_id: String,
    node_id: String,
    settings: RecordingSettings,
    expected_revision: u64,
) -> Result<RecordingSettings, String> {
    let gate = client.recordings.lock_commits_owned().await;
    let snapshot = client
        .request(Method::GET, routes::CONTROLS_SNAPSHOT, None)
        .await?;
    if !snapshot.is_success() {
        return Err("recorder settings unavailable".into());
    }
    recording::validate_target(&snapshot.body, &graph_id, &node_id, expected_revision)
        .map_err(|error| error.to_string())?;
    let recordings = Arc::clone(&client.recordings);
    let next = settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        recordings.commit(gate, (graph_id, node_id), next)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(settings)
}

impl ControlProxyClient {
    pub(super) fn recording_preferences(
        &self,
        graph: &str,
        node: &str,
    ) -> Result<RecordingSettings, String> {
        self.recordings.preferences(graph, node)
    }

    pub(super) async fn recording_status(
        &self,
        graph: &str,
        node: &str,
    ) -> Result<serde_json::Value, String> {
        let response = self
            .request(Method::GET, &routes::recorder(graph, node), None)
            .await?;
        if !response.is_success() {
            return Err(format!("recorder unavailable: {}", response.body));
        }
        Ok(response.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_desktop_core::recording::RecordingEncoding;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, RecordingStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = load_store(
            directory.path().join("preferences.json"),
            directory.path().to_path_buf(),
        )
        .unwrap();
        (directory, store)
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_keeps_commit_owned_and_reads_available_until_shutdown_can_drain() {
        let (directory, store) = fixture();
        let client = ControlProxyClient::with_socket(None);
        client.recordings.replace(store).unwrap();
        let mut settings = client.recording_preferences("g", "n").unwrap();
        settings.encoding = RecordingEncoding::Pcm24;
        let next = settings.clone();
        let recordings = Arc::clone(&client.recordings);
        let gate = client.recordings.lock_commits_owned().await;
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, proceed) = std::sync::mpsc::channel();
        let caller = tokio::spawn(async move {
            tauri::async_runtime::spawn_blocking(move || {
                ready.send(()).unwrap();
                proceed.recv().unwrap();
                recordings.commit(gate, ("g".into(), "n".into()), next)
            })
            .await
            .unwrap()
        });
        started.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            client.recording_preferences("g", "n").unwrap().encoding,
            RecordingEncoding::Pcm16
        );
        assert!(client.recordings.gate.try_lock().is_err());
        client.close_admission();
        release.send(()).unwrap();
        client.drain().await.unwrap();
        assert_eq!(client.recording_preferences("g", "n").unwrap(), settings);
        let loaded = load_store(
            directory.path().join("preferences.json"),
            directory.path().to_path_buf(),
        )
        .unwrap();
        assert_eq!(loaded.get("g", "n"), settings);
    }

    #[test]
    fn shared_settings_survive_restart_and_remain_target_specific() {
        let (directory, mut store) = fixture();
        let mut settings = store.get("graph", "recorder");
        assert_eq!(settings.encoding, RecordingEncoding::Pcm16);
        assert_eq!(settings.sample_rate, None);
        settings.sample_rate = Some(96_000);
        settings.encoding = RecordingEncoding::Pcm24;
        store
            .save("graph".into(), "recorder".into(), settings.clone())
            .unwrap();
        let restored = load_store(
            directory.path().join("preferences.json"),
            directory.path().to_path_buf(),
        )
        .unwrap();
        assert_eq!(restored.get("graph", "recorder"), settings);
        assert_eq!(
            restored.get("other", "recorder").encoding,
            RecordingEncoding::Pcm16
        );
        assert_eq!(restored.get("graph", "other").sample_rate, None);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejected_preferences_do_not_change_memory_or_durable_state() {
        let (directory, mut store) = fixture();
        let settings = store.get("g", "n");
        store
            .save("g".into(), "n".into(), settings.clone())
            .unwrap();
        let before = fs::read(directory.path().join("preferences.json")).unwrap();
        let mut invalid = settings.clone();
        invalid.sample_rate = Some(123);
        assert!(store.save("g".into(), "n".into(), invalid).is_err());
        assert_eq!(store.get("g", "n"), settings);
        assert_eq!(
            fs::read(directory.path().join("preferences.json")).unwrap(),
            before
        );
    }

    #[test]
    fn failed_atomic_replace_keeps_previous_in_memory_preferences() {
        let (directory, mut store) = fixture();
        fs::create_dir(directory.path().join("preferences.json")).unwrap();
        let before = store.get("g", "n");
        let mut settings = before.clone();
        settings.encoding = RecordingEncoding::Float32;
        assert!(store.save("g".into(), "n".into(), settings).is_err());
        assert_eq!(store.get("g", "n"), before);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn new_take_names_are_unique_uncreated_and_use_canonical_format() {
        let (_directory, store) = fixture();
        let settings = store.get("g", "n");
        let names = NativeTakeIdentities::default();
        let first = settings.start_request(names.next().unwrap()).unwrap();
        let second = settings.start_request(names.next().unwrap()).unwrap();
        assert_ne!(first["path"], second["path"]);
        assert!(!std::path::Path::new(first["path"].as_str().unwrap()).exists());
        assert_eq!(first["encoding"], "pcm16");
        assert_eq!(first["sampleRate"], serde_json::Value::Null);
        assert_eq!(
            PathBuf::from(first["path"].as_str().unwrap())
                .parent()
                .unwrap(),
            std::path::Path::new(&settings.folder)
        );
    }
}
