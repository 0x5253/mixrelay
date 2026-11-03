//! Control-side file transactions shared by graph catalogs and session settings.
//! Callers own validation and commit policy; this layer never negotiates audio.

use std::{
    env,
    fs::{self, File, OpenOptions},
    hash::{BuildHasher, RandomState},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;

/// Writes and syncs JSON beside its destination without replacing that destination.
/// The returned owner commits explicitly or removes only its staged file on drop.
/// # Errors
/// Returns directory, creation, serialization, write or sync errors.
pub fn stage_json(path: &Path, value: &impl Serialize) -> io::Result<StagedWrite> {
    stage_json_limited(path, value, u64::MAX)
}

/// Serializes once, enforcing the complete on-disk byte limit, including the newline.
/// A size or serialization failure cancels only the unpublished staging file.
/// # Errors
/// Reports serialization, size, creation or synchronization failures.
pub fn stage_json_limited(
    path: &Path,
    value: &impl Serialize,
    limit: u64,
) -> io::Result<StagedWrite> {
    stage_file(path, |output| {
        let mut writer = LimitedWriter {
            inner: output,
            remaining: limit,
        };
        serde_json::to_writer(&mut writer, value).map_err(|error| {
            io::Error::new(error.io_error_kind().unwrap_or(io::ErrorKind::Other), error)
        })?;
        writer.write_all(b"\n")
    })
}

/// Streams a document into a private, synced sibling file. The writer never
/// observes the destination; errors retain the previously committed document.
/// # Errors
/// Reports directory, creation, writer or synchronization failures.
pub fn stage_file(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<StagedWrite> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("document needs a directory"))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let (temporary, mut file) = create_temporary(parent)?;
    let staged = StagedWrite {
        temporary,
        destination: path.to_path_buf(),
    };
    // Establish cleanup before serialization, and close the file before cleanup
    // on failure. Some platforms cannot unlink an open staged file.
    let result = (|| {
        let mut writer = BufWriter::with_capacity(64 * 1024, &mut file);
        write(&mut writer)?;
        writer.flush()?;
        drop(writer);
        file.sync_all()
    })();
    drop(file);
    result?;
    Ok(staged)
}

struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "document exceeds size limit",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Sole owner of an unpublished file. Commit consumes the owner; cancellation
/// never removes or truncates the previously accepted destination.
#[must_use = "commit the staged document or drop it to cancel"]
#[derive(Debug)]
pub struct StagedWrite {
    temporary: PathBuf,
    destination: PathBuf,
}

impl StagedWrite {
    /// Atomically replaces the destination with the synced staged file.
    /// # Errors
    /// Returns a rename failure before commit. A later directory-sync failure is
    /// logged, not reported as rollback: the replacement has already happened.
    pub fn commit(self) -> io::Result<()> {
        fs::rename(&self.temporary, &self.destination)?;
        #[cfg(unix)]
        if let Some(parent) = self.destination.parent()
            && let Err(error) = File::open(parent).and_then(|directory| directory.sync_all())
        {
            tracing::warn!(%error, "MixRelay document directory sync failed after commit");
        }
        Ok(())
    }
}

impl Drop for StagedWrite {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.temporary)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "MixRelay staged document cleanup failed");
        }
    }
}

fn create_temporary(parent: &Path) -> io::Result<(PathBuf, File)> {
    for attempt in 0..16 {
        let ticket = RandomState::new().hash_one(attempt);
        let path = parent.join(format!(".mixrelay-write-{ticket:016x}.tmp"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "staged document names occupied",
    ))
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Resolves the application's configuration directory without creating it.
/// Returns an invalid override or unavailable user-directory error.
/// The engine's configuration directory (`MIXRELAY_CONFIG_DIR` when set).
///
/// # Errors
/// Fails when the directory cannot be determined from the environment.
pub fn config_directory() -> io::Result<PathBuf> {
    if env::var_os("MIXRELAY_CONFIG_DIR").is_some() {
        return absolute_env("MIXRELAY_CONFIG_DIR").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "MIXRELAY_CONFIG_DIR must be absolute",
            )
        });
    }
    #[cfg(target_os = "macos")]
    let base = absolute_env("HOME").map(|home| home.join("Library/Application Support/MixRelay"));
    #[cfg(not(target_os = "macos"))]
    let base = absolute_env("XDG_CONFIG_HOME")
        .or_else(|| absolute_env("HOME").map(|home| home.join(".config")))
        .map(|directory| directory.join("mixrelay"));
    base.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user configuration directory unavailable",
        )
    })
}

#[cfg(test)]
mod tests;
