//! Path validation, payload copy, and recoverable filesystem helpers.

use std::{
    fs::{self, File, Metadata, Permissions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::TryLockError,
};

use super::{
    APP_PAYLOAD_NAME, CreatedDirectory, DRIVER_PAYLOAD_NAME, FilesystemError, FilesystemIdentity,
    FilesystemStaged, HAL_PAYLOAD_NAME, InstalledTarget, LAUNCHD_PAYLOAD_NAME, MetadataFingerprint,
    REMOVAL_LAUNCHD_PAYLOAD_NAME, RUNTIME_ENGINE_PAYLOAD_NAME, RUNTIME_LAUNCHD_PAYLOAD_NAME,
    TargetKind, UNINSTALLER_PAYLOAD_NAME,
};
use crate::contract::OwnedTarget;

pub(super) fn validate_absolute_path(path: &Path) -> Result<(), FilesystemError> {
    if !path.is_absolute() {
        return Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path must be absolute",
        });
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "path must be normalized",
        });
    }
    Ok(())
}

pub(super) fn ensure_no_symlink_components(
    path: &Path,
    allow_missing: bool,
) -> Result<(), FilesystemError> {
    let mut current = PathBuf::new();
    let components = path.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_failure("inspect", &current, error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(FilesystemError::UnsafePath {
                path: current,
                reason: "path contains a symlink component",
            });
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(FilesystemError::UnexpectedKind {
                kind: "directory path component",
                path: current,
            });
        }
    }
    Ok(())
}

pub(super) fn require_directory(path: &Path, kind: &'static str) -> Result<(), FilesystemError> {
    let metadata = metadata(path, kind)?;
    if !metadata.is_dir() {
        return Err(FilesystemError::UnexpectedKind {
            kind,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

pub(super) fn metadata(path: &Path, kind: &'static str) -> Result<Metadata, FilesystemError> {
    fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            FilesystemError::Missing {
                kind,
                path: path.to_path_buf(),
            }
        } else {
            io_failure("inspect", path, error)
        }
    })
}

pub(super) fn target_kind(target: OwnedTarget) -> TargetKind {
    match target {
        OwnedTarget::App | OwnedTarget::Hal | OwnedTarget::Uninstaller => TargetKind::Directory,
        OwnedTarget::Driver
        | OwnedTarget::Launchd
        | OwnedTarget::RemovalLaunchd
        | OwnedTarget::RuntimeEngine
        | OwnedTarget::RuntimeLaunchd => TargetKind::File,
    }
}

pub(super) fn target_name(target: OwnedTarget) -> &'static str {
    match target {
        OwnedTarget::App => APP_PAYLOAD_NAME,
        OwnedTarget::Driver => DRIVER_PAYLOAD_NAME,
        OwnedTarget::Hal => HAL_PAYLOAD_NAME,
        OwnedTarget::Launchd => LAUNCHD_PAYLOAD_NAME,
        OwnedTarget::Uninstaller => UNINSTALLER_PAYLOAD_NAME,
        OwnedTarget::RemovalLaunchd => REMOVAL_LAUNCHD_PAYLOAD_NAME,
        OwnedTarget::RuntimeEngine => RUNTIME_ENGINE_PAYLOAD_NAME,
        OwnedTarget::RuntimeLaunchd => RUNTIME_LAUNCHD_PAYLOAD_NAME,
    }
}

pub(super) fn has_expected_kind(metadata: &Metadata, kind: TargetKind) -> bool {
    match kind {
        TargetKind::Directory => metadata.is_dir(),
        TargetKind::File => metadata.is_file(),
    }
}

pub(super) fn fingerprint(metadata: &Metadata) -> MetadataFingerprint {
    MetadataFingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    }
}

pub(super) fn identity(metadata: &Metadata) -> FilesystemIdentity {
    FilesystemIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

pub(super) fn validate_file_source(
    kind: &'static str,
    path: &Path,
    metadata: &Metadata,
) -> Result<(), FilesystemError> {
    if metadata.len() == 0 {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: path.to_path_buf(),
            reason: "file is empty",
        });
    }
    if matches!(kind, DRIVER_PAYLOAD_NAME | RUNTIME_ENGINE_PAYLOAD_NAME)
        && metadata.permissions().mode() & 0o111 == 0
    {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: path.to_path_buf(),
            reason: "Driver is not executable",
        });
    }
    Ok(())
}

pub(super) fn validate_bundle_source(
    kind: &'static str,
    path: &Path,
) -> Result<(), FilesystemError> {
    let executable = if kind == APP_PAYLOAD_NAME {
        path.join("Contents/MacOS/mixrelay-desktop")
    } else if kind == UNINSTALLER_PAYLOAD_NAME {
        path.join("Contents/MacOS/mixrelay-driver-uninstaller")
    } else {
        path.join("Contents/MacOS/MixRelayHAL")
    };
    let info = path.join("Contents/Info.plist");
    validate_regular_file(kind, &info)?;
    let executable_metadata = metadata(&executable, kind)?;
    if !executable_metadata.is_file()
        || executable_metadata.len() == 0
        || executable_metadata.permissions().mode() & 0o111 == 0
    {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: executable,
            reason: "bundle executable is missing, empty, or not executable",
        });
    }
    validate_tree(path)
}

fn validate_regular_file(kind: &'static str, path: &Path) -> Result<(), FilesystemError> {
    let metadata = metadata(path, kind)?;
    if !metadata.is_file() {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: path.to_path_buf(),
            reason: "expected regular file",
        });
    }
    Ok(())
}

fn validate_tree(root: &Path) -> Result<(), FilesystemError> {
    for entry in sorted_entries(root)? {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| io_failure("inspect", &path, error))?;
        if file_type.is_symlink() {
            return Err(FilesystemError::UnsafePath {
                path,
                reason: "artifact tree contains a symlink",
            });
        }
        if file_type.is_dir() {
            validate_tree(&path)?;
        } else if !file_type.is_file() {
            return Err(FilesystemError::UnexpectedKind {
                kind: "regular artifact tree entry",
                path,
            });
        }
    }
    Ok(())
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, FilesystemError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| io_failure("read directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_failure("read directory entry", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

pub(super) fn copy_tree(source: &Path, destination: &Path) -> Result<(), FilesystemError> {
    let source_metadata = metadata(source, "staged source")?;
    if source_metadata.is_dir() {
        create_directory(destination)?;
        for entry in sorted_entries(source)? {
            let path = entry.path();
            let target = destination.join(entry.file_name());
            let file_type = entry
                .file_type()
                .map_err(|error| io_failure("inspect", &path, error))?;
            if file_type.is_symlink() {
                return Err(FilesystemError::UnsafePath {
                    path,
                    reason: "artifact tree contains a symlink",
                });
            }
            copy_tree(&path, &target)?;
        }
        return Ok(());
    }
    if !source_metadata.is_file() {
        return Err(FilesystemError::UnexpectedKind {
            kind: "copy source",
            path: source.to_path_buf(),
        });
    }
    copy_regular_file(source, destination)
}

pub(super) fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), FilesystemError> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Read;

    // Source paths are not trusted while copying. Never block opening a swapped
    // FIFO or follow a replaced leaf symlink; validate the opened inode itself.
    let descriptor = rustix::fs::open(
        source,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| io_failure("open copy source", source, error.into()))?;
    let input = File::from(descriptor);
    let before = input
        .metadata()
        .map_err(|error| io_failure("inspect copy source", source, error))?;
    if !before.is_file() {
        return Err(FilesystemError::UnexpectedKind {
            kind: "regular copy source",
            path: source.to_path_buf(),
        });
    }
    ensure_parent_directory(destination)?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|error| io_failure("create private copy", destination, error))?;
    let copied = io::copy(&mut (&input).take(before.len()), &mut output)
        .map_err(|error| io_failure("copy", destination, error))?;
    let after = input
        .metadata()
        .map_err(|error| io_failure("recheck copy source", source, error))?;
    if copied != before.len() || fingerprint(&before) != fingerprint(&after) {
        return Err(FilesystemError::InvalidSource {
            kind: "copy source",
            path: source.to_path_buf(),
            reason: "source changed during bounded copy",
        });
    }
    let mode = if before.mode() & 0o111 == 0 {
        0o644
    } else {
        0o755
    };
    output
        .set_permissions(Permissions::from_mode(mode))
        .map_err(|error| io_failure("set copied permissions", destination, error))
}

pub(super) fn create_directory(path: &Path) -> Result<Option<File>, FilesystemError> {
    #[cfg(target_os = "macos")]
    if path == Path::new(crate::PRODUCT_DIRECTORY) {
        return crate::native::create_product_directory()
            .map(Some)
            .map_err(|source| {
                io_failure(
                    "create protected product folder",
                    path,
                    io::Error::other(source),
                )
            });
    }
    create_directory_with_mode(path, 0o755)?;
    Ok(None)
}

pub(super) fn create_private_directory(path: &Path) -> Result<(), FilesystemError> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|error| io_failure("create private staging", path, error))
}

fn create_directory_with_mode(path: &Path, mode: u32) -> Result<(), FilesystemError> {
    fs::create_dir(path).map_err(|error| io_failure("create directory", path, error))?;
    fs::set_permissions(path, Permissions::from_mode(mode))
        .map_err(|error| io_failure("set permissions", path, error))
}

fn ensure_parent_directory(path: &Path) -> Result<(), FilesystemError> {
    let parent = path.parent().ok_or_else(|| FilesystemError::UnsafePath {
        path: path.to_path_buf(),
        reason: "path has no parent",
    })?;
    require_directory(parent, "copy destination parent")
}

pub(super) fn create_missing_directories(
    path: &Path,
    created: &mut Vec<CreatedDirectory>,
    unidentified: &mut Vec<PathBuf>,
) -> Result<(), FilesystemError> {
    let mut missing = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(FilesystemError::UnexpectedKind {
                        kind: "destination parent directory",
                        path: current,
                    });
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(current.clone());
                current = current
                    .parent()
                    .map_or_else(PathBuf::new, Path::to_path_buf);
                if current.as_os_str().is_empty() {
                    return Err(FilesystemError::Missing {
                        kind: "destination root",
                        path: path.to_path_buf(),
                    });
                }
            }
            Err(error) => return Err(io_failure("inspect", &current, error)),
        }
    }
    for directory in missing.iter().rev() {
        unidentified.push(directory.clone());
        let descriptor = create_directory(directory)?;
        let metadata = match descriptor.map_or_else(
            || metadata(directory, "created destination directory"),
            |file| {
                file.metadata().map_err(|error| {
                    io_failure("inspect created product descriptor", directory, error)
                })
            },
        ) {
            Ok(metadata) => metadata,
            Err(cause) => {
                return Err(FilesystemError::CreatedDirectoryIdentityUnavailable {
                    path: directory.clone(),
                    cause: Box::new(cause),
                });
            }
        };
        created.push(CreatedDirectory {
            path: directory.clone(),
            identity: identity(&metadata),
        });
        unidentified.pop();
    }
    Ok(())
}

pub(super) fn validate_staged_payload(
    target: OwnedTarget,
    path: &Path,
) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, false)?;
    let metadata = metadata(path, "staged payload")?;
    if !has_expected_kind(&metadata, target_kind(target)) {
        return Err(FilesystemError::UnexpectedKind {
            kind: "staged payload",
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        validate_tree(path)
    } else {
        Ok(())
    }
}

pub(super) fn ensure_absent(path: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(_) => Err(FilesystemError::StagingOccupied {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_failure("inspect", path, error)),
    }
}

pub(super) fn rename_path(source: &Path, destination: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(source, false)?;
    ensure_no_symlink_components(destination, true)?;
    fs::rename(source, destination).map_err(|error| io_failure("rename", source, error))
}

fn remove_owned_path(path: &Path, target: OwnedTarget) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_failure("inspect", path, error)),
    };
    if !has_expected_kind(&metadata, target_kind(target)) {
        return Err(FilesystemError::UnexpectedKind {
            kind: "owned target during rollback",
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|error| io_failure("remove owned directory", path, error))
    } else {
        fs::remove_file(path).map_err(|error| io_failure("remove owned file", path, error))
    }
}

pub(super) fn remove_installed_path(
    path: &Path,
    installed: &InstalledTarget,
) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    let metadata = metadata(path, "installed target during rollback")?;
    if fingerprint(&metadata) != installed.fingerprint {
        return Err(FilesystemError::ConcurrentTargetChange {
            target: installed.target,
        });
    }
    remove_owned_path(path, installed.target)
}

pub(super) fn remove_created_directory(
    directory: &CreatedDirectory,
) -> Result<(), FilesystemError> {
    let metadata = metadata(&directory.path, "created destination directory")?;
    if identity(&metadata) != directory.identity || !metadata.is_dir() {
        return Err(FilesystemError::CreatedDirectoryChanged {
            path: directory.path.clone(),
        });
    }
    fs::remove_dir(&directory.path)
        .map_err(|error| io_failure("remove created directory", &directory.path, error))
}

fn remove_staging_root(path: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|error| io_failure("remove staging", path, error))
        }
        Ok(_) => Err(FilesystemError::UnexpectedKind {
            kind: "private staging directory",
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_failure("inspect", path, error)),
    }
}

pub(super) fn remove_staging_root_if_identity(
    path: &Path,
    expected: FilesystemIdentity,
) -> Result<(), FilesystemError> {
    let observed = match metadata(path, "private staging") {
        Ok(metadata) => identity(&metadata),
        Err(FilesystemError::Missing { .. }) => return Ok(()),
        Err(error) => return Err(error),
    };
    if observed != expected {
        return Err(FilesystemError::StagingIdentityChanged {
            path: path.to_path_buf(),
        });
    }
    remove_staging_root(path)
}

pub(super) fn ensure_staging_identity(staged: &FilesystemStaged) -> Result<(), FilesystemError> {
    let observed = identity(&metadata(&staged.staging_root, "private staging")?);
    if observed != staged.staging_identity {
        return Err(FilesystemError::StagingIdentityChanged {
            path: staged.staging_root.clone(),
        });
    }
    Ok(())
}

pub(super) fn is_within(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

pub(super) fn io_failure(
    operation: &'static str,
    path: &Path,
    source: io::Error,
) -> FilesystemError {
    FilesystemError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

pub(super) fn receipt_lock_error<T>(error: &TryLockError<T>) -> FilesystemError {
    match error {
        TryLockError::WouldBlock => FilesystemError::ReceiptBusy,
        TryLockError::Poisoned(_) => FilesystemError::ReceiptPoisoned,
    }
}
