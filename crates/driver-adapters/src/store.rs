//! JSON file persistence with private permissions and atomic replacement.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use mixrelay_driver_control::{
    DriverDeviceConfig, DriverError, JSON_LIMIT, SettingsStore, StoredSettings,
};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DriverStore {
    revision: u64,
    /// `false` keeps the retained devices unpublished until the user resumes the Driver.
    active: bool,
    devices: Vec<DriverDeviceConfig>,
}

/// JSON settings at a caller-selected path; construction does not access the filesystem.
#[derive(Debug)]
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    /// Selects the file used for subsequent loads and commits.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredDevices<'a> {
    revision: u64,
    active: bool,
    devices: &'a [DriverDeviceConfig],
}

impl SettingsStore for FileSettingsStore {
    fn load(&self) -> Result<Option<StoredSettings>, DriverError> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(1_048_577).read_to_end(&mut bytes)?;
        if bytes.len() > JSON_LIMIT {
            return Err(DriverError::Invalid("Driver store exceeds 1 MiB".into()));
        }
        let stored: DriverStore = serde_json::from_slice(&bytes)?;
        Ok(Some(StoredSettings {
            revision: stored.revision,
            active: stored.active,
            devices: stored.devices,
        }))
    }

    fn save(&self, settings: &StoredSettings) -> Result<(), DriverError> {
        let bytes = serde_json::to_vec(&StoredDevices {
            revision: settings.revision,
            active: settings.active,
            devices: &settings.devices,
        })?;
        if bytes.len() > JSON_LIMIT {
            return Err(DriverError::Invalid("Driver store exceeds 1 MiB".into()));
        }
        write_atomic(&self.path, &bytes)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), DriverError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary_path = parent.join(format!(
        ".mixrelay-driver-{}-{}.tmp",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary_path)?;
    let temporary = PendingFile(temporary_path);
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    Ok(())
}

struct PendingFile(PathBuf);

impl Drop for PendingFile {
    fn drop(&mut self) {
        // Only this unpublished sibling is owned here; the committed destination is untouched.
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests;
