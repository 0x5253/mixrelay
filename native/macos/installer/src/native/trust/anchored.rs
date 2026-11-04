//! Opens and creates fixed directories one component at a time without following links.

use std::{
    ffi::OsStr,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};

use rustix::{
    fs::{Gid, Mode, OFlags, Uid, fchmod, fchown, mkdirat, openat},
    io::Errno,
};

use super::{
    policy::{self, Inode, ROOT_DIRECTORY_MODE},
    require,
};
use crate::{
    directory::{open_child, open_directory},
    native::NativeError,
};

/// Reads the trust facts of an opened inode.
pub(super) fn inspect(
    file: &File,
    operation: &'static str,
    path: &Path,
) -> Result<Inode, NativeError> {
    file.metadata()
        .map(|metadata| Inode::of(&metadata))
        .map_err(|source| io_error(operation, path, source))
}

/// Reads the trust facts of a path without following a final link.
pub(super) fn inspect_path(operation: &'static str, path: &Path) -> Result<Inode, NativeError> {
    std::fs::symlink_metadata(path)
        .map(|metadata| Inode::of(&metadata))
        .map_err(|source| io_error(operation, path, source))
}

/// Like [`inspect_path`], but only `NotFound` reports an absent path.
pub(super) fn inspect_path_if_present(
    operation: &'static str,
    path: &Path,
) -> Result<Option<Inode>, NativeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(Inode::of(&metadata))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error(operation, path, source)),
    }
}

pub(super) fn open_root() -> Result<File, NativeError> {
    open_directory(Path::new("/"))
        .map_err(|source| io_error("open filesystem root", Path::new("/"), source))
}

/// Opens an absolute path whose every component is a protected root-owned directory.
pub(super) fn open_protected_directory(path: &Path) -> Result<File, NativeError> {
    require_absolute_normalized(path)?;
    let mut current = open_root()?;
    let mut walked = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(name) = component {
            walked.push(name);
            current = open_child_directory(&current, name, &walked)?;
            let inode = inspect(&current, "inspect destination path component", &walked)?;
            require(&walked, policy::path_component(&walked, inode))?;
        }
    }
    Ok(current)
}

pub(super) fn open_child_directory(
    parent: &File,
    name: impl AsRef<OsStr>,
    path: &Path,
) -> Result<File, NativeError> {
    open_child(parent, name.as_ref())
        .map_err(|source| io_error("open protected directory", path, source))
}

/// Opens a root-owned child directory, creating it as root:`group` 0755 when missing.
pub(super) fn open_or_create_root_directory(
    parent: &File,
    name: &str,
    path: &Path,
    group: Gid,
) -> Result<File, NativeError> {
    open_or_create_directory_as(parent, name, path, (Uid::ROOT, group))
}

/// Existing children are returned unchanged; callers validate them.
pub(super) fn open_or_create_directory_as(
    parent: &File,
    name: &str,
    path: &Path,
    owner: (Uid, Gid),
) -> Result<File, NativeError> {
    match open_child(parent, OsStr::new(name)) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_directory_as(parent, name, path, owner)
        }
        Err(source) => Err(io_error("open protected directory", path, source)),
    }
}

fn create_directory_as(
    parent: &File,
    name: &str,
    path: &Path,
    owner: (Uid, Gid),
) -> Result<File, NativeError> {
    let mode = Mode::from_raw_mode(ROOT_DIRECTORY_MODE);
    mkdirat(parent, name, mode)
        .map_err(|source| errno_error("create protected directory", path, source))?;
    let directory = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|source| errno_error("open newly created protected directory", path, source))?;
    fchown(&directory, Some(owner.0), Some(owner.1))
        .map_err(|source| errno_error("set protected directory ownership", path, source))?;
    fchmod(&directory, mode)
        .map_err(|source| errno_error("set protected directory mode", path, source))?;
    Ok(directory)
}

/// Requires the opened directory to be root-owned 0755 with the expected group.
pub(super) fn require_root_directory(
    path: &Path,
    directory: &File,
    expected_gid: Option<Gid>,
) -> Result<(), NativeError> {
    let inode = inspect(directory, "inspect lifecycle directory", path)?;
    require(
        path,
        policy::root_directory(inode, expected_gid.map(Gid::as_raw)),
    )
}

fn require_absolute_normalized(path: &Path) -> Result<(), NativeError> {
    let normalized = path
        .components()
        .all(|component| !matches!(component, Component::CurDir | Component::ParentDir));
    require(
        path,
        if path.is_absolute() && normalized {
            Ok(())
        } else {
            Err("native path must be absolute and normalized")
        },
    )
}

pub(super) fn io_error(operation: &'static str, path: &Path, source: io::Error) -> NativeError {
    NativeError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

pub(super) fn errno_error(operation: &'static str, path: &Path, source: Errno) -> NativeError {
    io_error(operation, path, io::Error::from(source))
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    };

    use mixrelay_test_support::FixtureDir;

    use super::*;

    #[test]
    fn protected_child_creation_and_existing_link_refusal_are_descriptor_anchored() {
        let root = FixtureDir::new("product-folder-create").expect("fixture");
        let parent = File::open(root.path()).expect("parent descriptor");
        let path = root.join("MixRelay");
        let owner = (rustix::process::geteuid(), rustix::process::getegid());
        let directory = open_or_create_directory_as(&parent, "MixRelay", &path, owner).unwrap();
        let metadata = directory.metadata().unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.mode() & 0o7777, 0o755);
        assert_eq!(
            (metadata.uid(), metadata.gid()),
            (owner.0.as_raw(), owner.1.as_raw())
        );
        let existing = open_or_create_directory_as(&parent, "MixRelay", &path, owner).unwrap();
        assert_eq!(existing.metadata().unwrap().ino(), metadata.ino());
        assert_eq!(
            require_root_directory(&path, &directory, None).is_ok(),
            owner.0 == Uid::ROOT,
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o775)).unwrap();
        assert!(require_root_directory(&path, &directory, None).is_err());
        let outside = root.join("outside");
        fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(open_or_create_directory_as(&parent, "MixRelay", &path, owner).is_err());
        assert!(outside.is_dir());
        assert_eq!(fs::metadata(outside).unwrap().mode() & 0o7777, 0o775);
    }

    #[test]
    fn relative_or_dotted_paths_are_refused_before_any_open() {
        for path in ["Library", "/Library/../tmp"] {
            assert!(matches!(
                open_protected_directory(Path::new(path)),
                Err(NativeError::Trust { reason, .. })
                    if reason == "native path must be absolute and normalized"
            ));
        }
    }
}
