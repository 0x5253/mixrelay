//! Destination parents created by a transaction, recorded by identity so rollback can
//! remove exactly those directories.

use std::{
    fs::{self, File, Permissions},
    io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{
    error::{FilesystemError, io_failure},
    paths::metadata,
    snapshot::{FilesystemIdentity, identity},
};

/// A directory this transaction created, with the inode it created.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CreatedDirectory {
    pub(super) path: PathBuf,
    pub(super) identity: FilesystemIdentity,
}

/// Creates one 0755 directory. The production product folder is instead published by
/// the privileged adapter, which returns its retained descriptor.
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
    fs::create_dir(path).map_err(|error| io_failure("create directory", path, error))?;
    fs::set_permissions(path, Permissions::from_mode(0o755))
        .map_err(|error| io_failure("set permissions", path, error))?;
    Ok(None)
}

pub(super) fn create_private_directory(path: &Path) -> Result<(), FilesystemError> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|error| io_failure("create private staging", path, error))
}

/// Creates every missing ancestor of `path`, outermost first.
///
/// Each path is recorded in `unidentified` before creation and moves to `created`
/// once its identity is read, so a failure in between still blocks rollback from
/// guessing which inode it made.
pub(super) fn create_missing_directories(
    path: &Path,
    created: &mut Vec<CreatedDirectory>,
    unidentified: &mut Vec<PathBuf>,
) -> Result<(), FilesystemError> {
    for directory in missing_ancestors(path)?.iter().rev() {
        unidentified.push(directory.clone());
        let descriptor = create_directory(directory)?;
        let metadata = descriptor
            .map_or_else(
                || metadata(directory, "created destination directory"),
                |file| {
                    file.metadata().map_err(|error| {
                        io_failure("inspect created product descriptor", directory, error)
                    })
                },
            )
            .map_err(
                |cause| FilesystemError::CreatedDirectoryIdentityUnavailable {
                    path: directory.clone(),
                    cause: Box::new(cause),
                },
            )?;
        created.push(CreatedDirectory {
            path: directory.clone(),
            identity: identity(&metadata),
        });
        unidentified.pop();
    }
    Ok(())
}

/// `path` and its missing ancestors, innermost first, up to an existing real directory.
fn missing_ancestors(path: &Path) -> Result<Vec<PathBuf>, FilesystemError> {
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
                return Ok(missing);
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
}

/// Removes a created directory only while it is still the same, now empty, inode.
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

#[cfg(test)]
mod tests {
    use mixrelay_test_support::FixtureDir;

    use super::*;

    #[test]
    fn missing_parents_are_created_outermost_first_and_recorded_by_identity() {
        let root = FixtureDir::new("directories-create").expect("fixture");
        let leaf = root.join("a/b/c");
        let (mut created, mut unidentified) = (Vec::new(), Vec::new());
        create_missing_directories(&leaf, &mut created, &mut unidentified).expect("create");
        let paths = created.iter().map(|d| d.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths, [root.join("a"), root.join("a/b"), leaf.clone()]);
        assert!(unidentified.is_empty());
        for directory in created.iter().rev() {
            remove_created_directory(directory).expect("remove own directory");
        }
        assert!(!root.join("a").exists());
    }

    #[test]
    fn a_replaced_created_directory_is_never_removed() {
        let root = FixtureDir::new("directories-replaced").expect("fixture");
        let path = root.join("created");
        let (mut created, mut unidentified) = (Vec::new(), Vec::new());
        create_missing_directories(&path, &mut created, &mut unidentified).expect("create");
        // The original stays alive, so the replacement cannot reuse its inode number.
        fs::rename(&path, root.join("original")).expect("move original aside");
        fs::create_dir(&path).expect("replacement");
        assert!(matches!(
            remove_created_directory(&created[0]),
            Err(FilesystemError::CreatedDirectoryChanged { .. })
        ));
        assert!(path.is_dir());
    }

    #[test]
    fn an_existing_file_or_symlinked_parent_stops_creation() {
        let root = FixtureDir::new("directories-blocked").expect("fixture");
        let file = root.join("file");
        fs::write(&file, b"x").expect("file");
        let link = root.join("link");
        std::os::unix::fs::symlink(root.path(), &link).expect("link");
        for blocked in [file, link.join("child")] {
            let (mut created, mut unidentified) = (Vec::new(), Vec::new());
            assert!(matches!(
                create_missing_directories(&blocked, &mut created, &mut unidentified),
                Err(FilesystemError::UnexpectedKind { .. })
            ));
            assert!(created.is_empty() && unidentified.is_empty());
        }
    }
}
