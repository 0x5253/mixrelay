//! Descriptor-relative directory operations at writable namespace boundaries.

use std::{ffi::OsStr, fs::File, io, path::Path};

use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, open, openat, renameat_with};

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    open(path, directory_flags(), Mode::empty())
        .map(File::from)
        .map_err(Into::into)
}

pub(crate) fn open_child(parent: &File, name: &OsStr) -> io::Result<File> {
    openat(parent, name, directory_flags(), Mode::empty())
        .map(File::from)
        .map_err(Into::into)
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// Both parents must be retained descriptors; neither leaf is followed or replaced.
pub(crate) fn rename_exclusive(
    source_parent: &File,
    source: &OsStr,
    destination_parent: &File,
    destination: &OsStr,
) -> io::Result<()> {
    // rustix maps NOREPLACE to renameatx_np(RENAME_EXCL) on macOS.
    renameat_with(
        source_parent,
        source,
        destination_parent,
        destination,
        RenameFlags::NOREPLACE,
    )
    .map_err(Into::into)
}

/// Creates an exclusive holding directory under an already protected parent.
/// Occupied holding directories retain evidence and are never reused or cleaned.
pub(crate) fn create_private_child(parent: &File, name: &OsStr) -> io::Result<File> {
    mkdirat(parent, name, Mode::RWXU)?;
    open_child(parent, name)
}
