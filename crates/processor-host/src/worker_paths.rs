//! Private owned processor IPC directories selected explicitly at the native boundary.
use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// One exclusive namespace shared by configuration, mappings and retained control readers.
pub(super) struct WorkerPaths {
    directory: Arc<tempfile::TempDir>,
    shared: PathBuf,
    config: PathBuf,
}
impl WorkerPaths {
    pub(super) fn new_in(parent: &Path) -> io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("processor-")
            .tempdir_in(parent)?;
        let shared = directory.path().join("transport.audio");
        let config = directory.path().join("config.json");
        Ok(Self {
            directory: Arc::new(directory),
            shared,
            config,
        })
    }
    pub(super) fn shared(&self) -> &Path {
        &self.shared
    }
    pub(super) fn config(&self) -> &Path {
        &self.config
    }
    pub(super) fn directory(&self) -> Arc<tempfile::TempDir> {
        Arc::clone(&self.directory)
    }
}

pub(super) fn worker_paths() -> io::Result<WorkerPaths> {
    let directory = worker_directory()?;
    private_directory(&directory)?;
    WorkerPaths::new_in(&directory)
}

pub(super) fn worker_directory() -> io::Result<PathBuf> {
    cache_directory_for(std::env::consts::OS, |key| std::env::var_os(key))
        .map(|path| path.join("processor-workers"))
}

fn cache_directory_for(
    os: &str,
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> io::Result<PathBuf> {
    let (key, suffix) = match os {
        "macos" => ("HOME", "Library/Caches/MixRelay"),
        "linux" => {
            if let Some(path) = environment("XDG_CACHE_HOME").filter(|path| !path.is_empty()) {
                return absolute(path, "XDG_CACHE_HOME").map(|path| path.join("MixRelay"));
            }
            ("HOME", ".cache/MixRelay")
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "processor cache is unsupported on this OS",
            ));
        }
    };
    let value = environment(key).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{key} is missing; processor cache unavailable"),
        )
    })?;
    absolute(value, key).map(|path| path.join(suffix))
}

fn absolute(value: OsString, key: &str) -> io::Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{key} must name an absolute per-user directory"),
        ));
    }
    Ok(path)
}

pub(super) fn private_directory(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "processor cache must be a real directory, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: geteuid has no arguments or pointer lifetimes. Do not chmod
        // another user's directory if environment/configuration points there.
        if metadata.uid() != unsafe { geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "processor cache is not owned by this user",
            ));
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
