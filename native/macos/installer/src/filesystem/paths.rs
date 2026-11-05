//! Path-level safety checks: absolute, normalized, symlink-free and of the expected kind.

use std::{
    fs::{self, Metadata},
    io,
    path::{Component, Path, PathBuf},
};

use super::error::{FilesystemError, io_failure};

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

/// Rejects any existing symlink on the way to `path`; `allow_missing` stops at the first gap.
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

/// Reads metadata without following a final link; absence becomes [`FilesystemError::Missing`].
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

/// Reads metadata without following a final link; `None` only for `NotFound`.
pub(super) fn metadata_if_present(
    path: &Path,
    operation: &'static str,
) -> Result<Option<Metadata>, FilesystemError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(operation, path, error)),
    }
}

/// Whether `path` is `parent` or lies beneath it.
pub(super) fn is_within(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

/// Requires a free private slot; an occupied one means earlier evidence must be kept.
pub(super) fn ensure_absent(path: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    match metadata_if_present(path, "inspect")? {
        Some(_) => Err(FilesystemError::StagingOccupied {
            path: path.to_path_buf(),
        }),
        None => Ok(()),
    }
}

pub(super) fn rename_path(source: &Path, destination: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(source, false)?;
    ensure_no_symlink_components(destination, true)?;
    fs::rename(source, destination).map_err(|error| io_failure("rename", source, error))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use mixrelay_test_support::FixtureDir;

    use super::*;

    #[test]
    fn relative_and_dotted_paths_are_unsafe() {
        assert!(validate_absolute_path(Path::new("/Library/Audio")).is_ok());
        for path in ["Library/Audio", "/Library/../tmp"] {
            assert!(matches!(
                validate_absolute_path(Path::new(path)),
                Err(FilesystemError::UnsafePath { .. })
            ));
        }
    }

    #[test]
    fn a_symlink_anywhere_on_the_path_is_rejected_at_that_component() {
        let root = FixtureDir::new("paths-symlink").expect("fixture");
        let real = root.join("real");
        fs::create_dir_all(real.join("child")).expect("real tree");
        let alias = root.join("alias");
        symlink(&real, &alias).expect("alias");
        assert!(ensure_no_symlink_components(&real.join("child"), false).is_ok());
        assert!(matches!(
            ensure_no_symlink_components(&alias.join("child"), false),
            Err(FilesystemError::UnsafePath { path, .. }) if path == alias
        ));
        assert!(matches!(
            ensure_no_symlink_components(&alias.join("missing"), true),
            Err(FilesystemError::UnsafePath { path, .. }) if path == alias
        ));
    }

    #[test]
    fn missing_tails_are_allowed_only_when_requested_and_files_cannot_be_parents() {
        let root = FixtureDir::new("paths-missing").expect("fixture");
        let missing = root.join("missing/leaf");
        assert!(ensure_no_symlink_components(&missing, true).is_ok());
        assert!(matches!(
            ensure_no_symlink_components(&missing, false),
            Err(FilesystemError::Io { .. })
        ));
        let file = root.join("file");
        fs::write(&file, b"x").expect("file");
        assert!(matches!(
            ensure_no_symlink_components(&file.join("child"), true),
            Err(FilesystemError::UnexpectedKind { path, .. }) if path == file
        ));
    }

    #[test]
    fn containment_is_by_path_component_not_string_prefix() {
        assert!(is_within(Path::new("/a/b"), Path::new("/a")));
        assert!(is_within(Path::new("/a"), Path::new("/a")));
        assert!(!is_within(Path::new("/ab"), Path::new("/a")));
        assert!(!is_within(Path::new("/a"), Path::new("/a/b")));
    }
}
