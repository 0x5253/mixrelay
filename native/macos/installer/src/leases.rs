//! Validated ownership locks for the macOS installer transaction boundary.
//!
//! This adapter opens the fixed lifecycle lease inodes and retains advisory
//! locks until [`LifecycleLeaseGuard`] is dropped. It deliberately accepts an
//! explicit directory instead of discovering or creating a privileged
//! production path so temporary unprivileged fixtures can exercise the same
//! lock ordering.
//!
//! Directory components are opened without following symlinks, and lease opens
//! are relative to the retained directory descriptor. The privileged caller must
//! still establish root-owned parent directories and selected-user lease-file
//! ownership: this adapter does not prevent an authorized directory writer from
//! unlinking a locked inode or changing ownership after validation.

use std::{
    fs::{self, File, Metadata},
    io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use rustix::fs::{Mode, OFlags, openat};
use thiserror::Error;

use crate::backend::LeaseKind;

const APP_LEASE_NAME: &str = "app.owner";
const ENGINE_LEASE_NAME: &str = "engine.owner";
const INSTALLER_LEASE_NAME: &str = "installer.owner";
const LEASE_FILE_MODE: Mode = Mode::RUSR
    .union(Mode::WUSR)
    .union(Mode::RGRP)
    .union(Mode::ROTH);

/// A lifecycle directory whose path and current directory inode satisfy the
/// unprivileged adapter's validation rules.
///
/// Lease opens remain anchored to this directory inode. Opening this value does
/// not establish privileged ownership; the native installer must exclude other
/// directory writers throughout the transaction.
#[derive(Debug)]
pub struct LifecycleDirectory {
    path: PathBuf,
    directory: File,
}

impl LifecycleDirectory {
    /// Validates an explicit lifecycle directory without creating it.
    ///
    /// The path must be absolute and normalized, every existing path
    /// component must be non-symlink, the final component must be a directory,
    /// and the directory must not be group/other writable or non-searchable.
    ///
    /// # Errors
    /// Returns [`LeaseError`] when path or metadata validation fails.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, LeaseError> {
        let path = path.into();
        validate_absolute_path(&path)?;
        validate_path_components(&path)?;
        let metadata = symlink_metadata(&path, "lifecycle directory")?;
        if !metadata.is_dir() {
            return Err(LeaseError::UnexpectedKind {
                path,
                expected: "directory",
            });
        }
        validate_directory_mode(&path, &metadata)?;
        let directory = open_directory(&path)?;
        let opened = directory.metadata().map_err(|source| LeaseError::Io {
            kind: LeaseKind::Installer,
            path: path.clone(),
            source,
        })?;
        if (metadata.dev(), metadata.ino()) != (opened.dev(), opened.ino()) {
            return Err(LeaseError::UnsafePath {
                path,
                reason: "directory changed while opening",
            });
        }
        Ok(Self { path, directory })
    }

    /// Returns the explicitly validated lifecycle directory path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Acquires shared app/engine locks followed by the exclusive installer lock.
    ///
    /// The acquisition order is fixed. If any later lock is busy or fails,
    /// already acquired file handles are dropped before the error is returned.
    /// Existing lease inodes are opened in place and are never replaced.
    /// Missing lease files may be created for an initial-install fixture; the
    /// resulting inode remains in place after a failed acquisition.
    ///
    /// # Errors
    /// Returns [`LeaseError::Busy`] for lock contention, or a validation/I/O
    /// error for an unusable lifecycle inode.
    pub fn acquire(&self) -> Result<LifecycleLeaseGuard, LeaseError> {
        let current = Self::open(self.path.clone())?;
        let previous = self.directory.metadata().map_err(|source| LeaseError::Io {
            kind: LeaseKind::Installer,
            path: self.path.clone(),
            source,
        })?;
        let now = current
            .directory
            .metadata()
            .map_err(|source| LeaseError::Io {
                kind: LeaseKind::Installer,
                path: self.path.clone(),
                source,
            })?;
        if (previous.dev(), previous.ino()) != (now.dev(), now.ino()) {
            return Err(LeaseError::UnsafePath {
                path: self.path.clone(),
                reason: "lifecycle directory was replaced",
            });
        }
        let app = self.acquire_lock(LeaseKind::App, false)?;
        let engine = self.acquire_lock(LeaseKind::Engine, false)?;
        let installer = self.acquire_lock(LeaseKind::Installer, true)?;
        Ok(LifecycleLeaseGuard {
            _app: app,
            _engine: engine,
            _installer: installer,
        })
    }

    fn acquire_lock(&self, kind: LeaseKind, exclusive: bool) -> Result<LifecycleLock, LeaseError> {
        let path = self.lease_path(kind);
        let file = open_lease_file(&self.directory, lease_name(kind), &path, kind)?;
        let result = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        result.map_err(|error| match error {
            fs::TryLockError::WouldBlock => LeaseError::Busy { kind },
            fs::TryLockError::Error(source) => LeaseError::Io { kind, path, source },
        })?;
        Ok(LifecycleLock(file))
    }

    fn lease_path(&self, kind: LeaseKind) -> PathBuf {
        self.path.join(lease_name(kind))
    }
}

fn lease_name(kind: LeaseKind) -> &'static str {
    match kind {
        LeaseKind::App => APP_LEASE_NAME,
        LeaseKind::Engine => ENGINE_LEASE_NAME,
        LeaseKind::Installer => INSTALLER_LEASE_NAME,
    }
}

/// Retained app/engine shared and installer-exclusive lifecycle locks.
///
/// Dropping this guard releases its locks before closing the files, even when
/// a concurrent fork has inherited descriptors that remain open until exec.
/// It contains no path-removal or installation behavior.
#[derive(Debug)]
pub struct LifecycleLeaseGuard {
    _app: LifecycleLock,
    _engine: LifecycleLock,
    _installer: LifecycleLock,
}

#[derive(Debug)]
struct LifecycleLock(File);

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        // CLOEXEC cannot close a forked descriptor until the child reaches exec.
        // The guard owns this lock, so release it at its own lifetime boundary.
        let _ = self.0.unlock();
    }
}

/// Errors from lifecycle-directory validation or nonblocking lease acquisition.
#[derive(Debug, Error)]
pub enum LeaseError {
    /// A path was relative, unnormalized, or contained a symlink component.
    #[error("unsafe lifecycle path {path}: {reason}")]
    UnsafePath {
        /// The rejected path or component.
        path: PathBuf,
        /// The validation that failed.
        reason: &'static str,
    },
    /// A required lifecycle path did not exist.
    #[error("missing lifecycle {kind}: {path}")]
    Missing {
        /// The missing path category.
        kind: &'static str,
        /// The missing path.
        path: PathBuf,
    },
    /// A lifecycle path had a different inode kind than required.
    #[error("unexpected lifecycle kind at {path}; expected {expected}")]
    UnexpectedKind {
        /// The path with the wrong kind.
        path: PathBuf,
        /// The required inode kind.
        expected: &'static str,
    },
    /// A lifecycle path had permissions outside the safe adapter policy.
    #[error("unsafe lifecycle mode at {path}: {mode:o} ({reason})")]
    UnsafeMode {
        /// The path whose mode was rejected.
        path: PathBuf,
        /// The permission bits observed.
        mode: u32,
        /// The failed mode invariant.
        reason: &'static str,
    },
    /// Another owner held one of the fixed advisory locks.
    #[error("lifecycle lease is busy: {kind:?}")]
    Busy {
        /// The lease that could not be acquired.
        kind: LeaseKind,
    },
    /// An operating-system operation on a lifecycle lease failed.
    #[error("lifecycle lease I/O failed for {kind:?} at {path}: {source}")]
    Io {
        /// The lease involved in the operation.
        kind: LeaseKind,
        /// The affected path.
        path: PathBuf,
        /// The operating-system failure.
        #[source]
        source: io::Error,
    },
}

fn validate_absolute_path(path: &Path) -> Result<(), LeaseError> {
    if !path.is_absolute() {
        return Err(LeaseError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path must be absolute",
        });
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(LeaseError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path must be normalized",
        });
    }
    Ok(())
}

fn validate_path_components(path: &Path) -> Result<(), LeaseError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = symlink_metadata(&current, "lifecycle path component")?;
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath {
                path: current,
                reason: "path contains a symlink component",
            });
        }
        if !metadata.is_dir() {
            return Err(LeaseError::UnexpectedKind {
                path: current,
                expected: "directory path component",
            });
        }
    }
    Ok(())
}

fn symlink_metadata(path: &Path, kind: &'static str) -> Result<Metadata, LeaseError> {
    fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            LeaseError::Missing {
                kind,
                path: path.to_path_buf(),
            }
        } else {
            LeaseError::Io {
                kind: LeaseKind::Installer,
                path: path.to_path_buf(),
                source,
            }
        }
    })
}

fn validate_directory_mode(path: &Path, metadata: &Metadata) -> Result<(), LeaseError> {
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o022 != 0 {
        return Err(LeaseError::UnsafeMode {
            path: path.to_path_buf(),
            mode,
            reason: "directory must not be group/other writable",
        });
    }
    if mode & 0o111 == 0 {
        return Err(LeaseError::UnsafeMode {
            path: path.to_path_buf(),
            mode,
            reason: "directory must be searchable",
        });
    }
    Ok(())
}

fn open_directory(path: &Path) -> Result<File, LeaseError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let failure = |source| LeaseError::Io {
        kind: LeaseKind::Installer,
        path: path.to_path_buf(),
        source: io::Error::from(source),
    };
    let mut current = rustix::fs::open("/", flags, Mode::empty()).map_err(failure)?;
    for component in path.components() {
        if let Component::Normal(name) = component {
            current = openat(&current, name, flags, Mode::empty()).map_err(failure)?;
        }
    }
    Ok(File::from(current))
}

/// Opens `name` inside `directory`; `path` is that file's full path for errors.
fn open_lease_file(
    directory: &File,
    name: &str,
    path: &Path,
    kind: LeaseKind,
) -> Result<File, LeaseError> {
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let opened = match openat(directory, name, flags, Mode::empty()) {
        Err(rustix::io::Errno::NOENT) => match openat(
            directory,
            name,
            flags | OFlags::CREATE | OFlags::EXCL,
            LEASE_FILE_MODE,
        ) {
            Err(rustix::io::Errno::EXIST) => openat(directory, name, flags, Mode::empty()),
            result => result,
        },
        result => result,
    };
    let file = File::from(opened.map_err(|source| {
        if source == rustix::io::Errno::LOOP {
            LeaseError::UnsafePath {
                path: path.to_path_buf(),
                reason: "lease path is a symlink",
            }
        } else if source == rustix::io::Errno::ISDIR {
            LeaseError::UnexpectedKind {
                path: path.to_path_buf(),
                expected: "regular lease file",
            }
        } else {
            LeaseError::Io {
                kind,
                path: path.to_path_buf(),
                source: io::Error::from(source),
            }
        }
    })?);
    let metadata = file.metadata().map_err(|source| LeaseError::Io {
        kind,
        path: path.to_path_buf(),
        source,
    })?;
    validate_lease_metadata(path, &metadata)?;
    Ok(file)
}

fn validate_lease_metadata(path: &Path, metadata: &Metadata) -> Result<(), LeaseError> {
    if metadata.file_type().is_symlink() {
        return Err(LeaseError::UnsafePath {
            path: path.to_path_buf(),
            reason: "lease path is a symlink",
        });
    }
    if !metadata.is_file() {
        return Err(LeaseError::UnexpectedKind {
            path: path.to_path_buf(),
            expected: "regular lease file",
        });
    }
    if metadata.nlink() != 1 {
        return Err(LeaseError::UnsafePath {
            path: path.to_path_buf(),
            reason: "lease inode must have exactly one link",
        });
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o022 != 0 || mode & 0o111 != 0 {
        return Err(LeaseError::UnsafeMode {
            path: path.to_path_buf(),
            mode,
            reason: "lease file must not be group/other writable or executable",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
