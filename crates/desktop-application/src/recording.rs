//! Next-take preferences, eligibility, and request preparation.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// One MiB bounds the desktop recording preference document.
pub const FILE_LIMIT: usize = 1_048_576;

/// Atomic storage of one preference document.
pub trait PreferencesRepository: std::fmt::Debug + Send + Sync {
    /// Returns no document for a first launch; implementations bound reads to `FILE_LIMIT + 1`.
    /// # Errors
    /// Returns storage read failures; a missing document is represented by `None`.
    fn read(&self) -> std::io::Result<Option<Vec<u8>>>;
    /// Replaces the whole document atomically or preserves the previous one.
    /// # Errors
    /// Returns a storage failure without publishing a partial replacement.
    fn replace(&self, bytes: &[u8]) -> std::io::Result<()>;
}
/// Native directory observations needed when saving user preferences.
pub trait RecordingDirectories: std::fmt::Debug + Send + Sync {
    /// Whether a selected folder currently exists.
    fn exists(&self, path: &Path) -> bool;
}
/// Caller-owned identity for one new take; no file is created during preparation.
#[derive(Clone, Copy, Debug)]
pub struct TakeIdentity {
    pub nanos: u128,
    pub owner: u32,
    pub sequence: u64,
}

/// Obtains unique names without giving policy access to clocks or process state.
pub trait TakeIdentities: Send + Sync {
    /// Reserves the next take identity.
    /// # Errors
    /// Returns identity clock failure or sequence exhaustion.
    fn next(&self) -> Result<TakeIdentity, RecordingError>;
}

/// Preference, target, and identity failures with their original storage/format causes.
#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("recording preferences too large")]
    TooLarge,
    #[error("unsupported recording preferences version")]
    UnsupportedVersion,
    #[error("recording folder must be valid Unicode")]
    UnicodeFolder,
    #[error("recording folder must be absolute")]
    AbsoluteFolder,
    #[error("unsupported recording sample rate")]
    SampleRate,
    #[error("select an existing recording folder")]
    MissingFolder,
    #[error("recording preferences not initialized")]
    Uninitialized,
    #[error("graph changed before recording settings were saved")]
    RevisionChanged,
    #[error("recorder no longer exists")]
    MissingRecorder,
    #[error("stop recording before changing recording settings")]
    RecorderActive,
    #[error("recording options belong to the app recorder settings")]
    OptionsInTransport,
    #[error("invalid recorder action")]
    InvalidAction,
    #[error("recording take identity is exhausted")]
    IdentityExhausted,
    #[error("could not read the recording take clock")]
    IdentityClock(#[source] std::time::SystemTimeError),
}

/// Settings for the next take. An active take retains its own immutable format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingSettings {
    pub folder: String,
    pub sample_rate: Option<u32>,
    pub encoding: RecordingEncoding,
}

/// Explicit UI defaults must not depend on the engine's omitted-value defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingEncoding {
    Pcm16,
    Pcm24,
    Float32,
}

type Entries = BTreeMap<String, BTreeMap<String, RecordingSettings>>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPreferences {
    version: u32,
    entries: Entries,
}

/// The mutex owner serializes durable edits with preparing and submitting a new take.
#[derive(Debug, Default, Clone)]
pub struct RecordingStore {
    repository: Option<Arc<dyn PreferencesRepository>>,
    directories: Option<Arc<dyn RecordingDirectories>>,
    default_folder: String,
    entries: Entries,
}

impl RecordingStore {
    /// Loads validated preferences through the supplied repository and directory view.
    /// # Errors
    /// Returns storage, format, version, size, and folder validation failures.
    pub fn load(
        repository: Arc<dyn PreferencesRepository>,
        directories: Arc<dyn RecordingDirectories>,
        default_folder: PathBuf,
    ) -> Result<Self, RecordingError> {
        let entries = match repository.read()? {
            Some(bytes) => {
                if bytes.len() > FILE_LIMIT {
                    return Err(RecordingError::TooLarge);
                }
                let saved: SavedPreferences = serde_json::from_slice(&bytes)?;
                if saved.version != 1 {
                    return Err(RecordingError::UnsupportedVersion);
                }
                for settings in saved.entries.values().flat_map(BTreeMap::values) {
                    settings.validate()?;
                }
                saved.entries
            }
            None => BTreeMap::new(),
        };
        let default_folder = default_folder
            .into_os_string()
            .into_string()
            .map_err(|_| RecordingError::UnicodeFolder)?;
        if !Path::new(&default_folder).is_absolute() {
            return Err(RecordingError::AbsoluteFolder);
        }
        Ok(Self {
            repository: Some(repository),
            directories: Some(directories),
            default_folder,
            entries,
        })
    }

    /// Returns target-specific next-take preferences or the configured app defaults.
    #[must_use]
    pub fn get(&self, graph: &str, node: &str) -> RecordingSettings {
        self.entries
            .get(graph)
            .and_then(|nodes| nodes.get(node))
            .cloned()
            .unwrap_or_else(|| RecordingSettings {
                folder: self.default_folder.clone(),
                sample_rate: None,
                encoding: RecordingEncoding::Pcm16,
            })
    }

    /// Commits durable preferences before publishing them to readers.
    /// # Errors
    /// Returns validation or storage failure before publishing new preferences.
    pub fn save(
        &mut self,
        graph: String,
        node: String,
        settings: RecordingSettings,
    ) -> Result<(), RecordingError> {
        settings.validate()?;
        let directories = self
            .directories
            .as_ref()
            .ok_or(RecordingError::Uninitialized)?;
        if !directories.exists(Path::new(&settings.folder)) {
            return Err(RecordingError::MissingFolder);
        }
        let repository = self
            .repository
            .as_ref()
            .ok_or(RecordingError::Uninitialized)?;
        let mut entries = self.entries.clone();
        entries.entry(graph).or_default().insert(node, settings);
        let bytes = serde_json::to_vec(&SavedPreferences {
            version: 1,
            entries: entries.clone(),
        })?;
        if bytes.len() > FILE_LIMIT {
            return Err(RecordingError::TooLarge);
        }
        repository.replace(&bytes)?;
        self.entries = entries;
        Ok(())
    }
}

impl RecordingSettings {
    /// Checks the folder representation and supported sample-rate catalog.
    /// # Errors
    /// Rejects nonabsolute or invalid folders and unsupported sample rates.
    pub fn validate(&self) -> Result<(), RecordingError> {
        if !Path::new(&self.folder).is_absolute() || self.folder.contains('\0') {
            return Err(RecordingError::AbsoluteFolder);
        }
        if self
            .sample_rate
            .is_some_and(|rate| !mixrelay_audio_io::formats::SAMPLE_RATES.contains(&rate))
        {
            return Err(RecordingError::SampleRate);
        }
        Ok(())
    }

    /// Prepares a unique uncreated take path from the caller-owned identity source.
    /// # Errors
    /// Returns preference validation failure before preparing a take path.
    pub fn start_request(&self, identity: TakeIdentity) -> Result<Value, RecordingError> {
        self.validate()?;
        let TakeIdentity {
            nanos,
            owner,
            sequence,
        } = identity;
        let path =
            PathBuf::from(&self.folder).join(format!("MixRelay-{nanos}-{owner}-{sequence}.wav"));
        Ok(
            serde_json::json!({"action":"record", "path":path, "sampleRate":self.sample_rate, "encoding":self.encoding}),
        )
    }
}
/// # Errors
/// Rejects stale revisions, missing recorders, and active takes.
pub fn validate_target(
    snapshot: &serde_json::Value,
    graph: &str,
    node: &str,
    revision: u64,
) -> Result<(), RecordingError> {
    if snapshot["revision"].as_u64() != Some(revision) {
        return Err(RecordingError::RevisionChanged);
    }
    let target = snapshot["graphs"]
        .as_array()
        .and_then(|graphs| graphs.iter().find(|item| item["id"] == graph))
        .and_then(|graph| graph["nodes"].as_array())
        .and_then(|nodes| nodes.iter().find(|item| item["nodeId"] == node))
        .filter(|node| node["capabilities"]["recorder"] == true)
        .ok_or(RecordingError::MissingRecorder)?;
    if !matches!(
        target["recorder"]["state"].as_str(),
        Some("stopped" | "failed")
    ) {
        return Err(RecordingError::RecorderActive);
    }
    Ok(())
}

/// Applies shared next-take format only to a new recording; resume keeps the active take.
/// # Errors
/// Rejects caller-supplied recording options, invalid actions, or take preparation failure.
pub fn prepare_transport(
    action: &Value,
    status: &Value,
    settings: &RecordingSettings,
    identities: &impl TakeIdentities,
) -> Result<Value, RecordingError> {
    if ["path", "sampleRate", "encoding"]
        .iter()
        .any(|key| !action[key].is_null())
    {
        return Err(RecordingError::OptionsInTransport);
    }
    match action["action"].as_str() {
        Some("record") if status["state"] != "paused" => settings.start_request(identities.next()?),
        Some(action @ ("record" | "pause" | "stop")) => Ok(serde_json::json!({"action":action})),
        _ => Err(RecordingError::InvalidAction),
    }
}

#[cfg(test)]
mod tests;
