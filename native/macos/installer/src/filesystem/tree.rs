//! Artifact trees: symlink-free validation and a bounded, non-following copy.

use std::{
    fs::{self, File, Permissions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use rustix::fs::{Mode, OFlags};

use super::{
    directories::create_directory,
    error::{FilesystemError, io_failure},
    paths::{metadata, require_directory},
    snapshot::fingerprint,
};

/// Requires every entry under `root` to be a regular file or directory.
pub(super) fn validate_tree(root: &Path) -> Result<(), FilesystemError> {
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

/// Copies a file or directory tree into a new destination; nothing existing is overwritten.
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

/// Copies one regular file, normalizing its mode to 0755 when executable and 0644 otherwise.
pub(super) fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), FilesystemError> {
    let input = open_copy_source(source)?;
    let before = input
        .metadata()
        .map_err(|error| io_failure("inspect copy source", source, error))?;
    if !before.is_file() {
        return Err(FilesystemError::UnexpectedKind {
            kind: "regular copy source",
            path: source.to_path_buf(),
        });
    }
    require_directory(parent(destination)?, "copy destination parent")?;
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

/// Source paths are not trusted while copying. Never block opening a swapped FIFO
/// or follow a replaced leaf symlink; callers validate the opened inode itself.
fn open_copy_source(source: &Path) -> Result<File, FilesystemError> {
    rustix::fs::open(
        source,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| io_failure("open copy source", source, error.into()))
}

fn parent(path: &Path) -> Result<&Path, FilesystemError> {
    path.parent().ok_or_else(|| FilesystemError::UnsafePath {
        path: path.to_path_buf(),
        reason: "path has no parent",
    })
}
