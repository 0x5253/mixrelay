//! Reclaims only the fixed private transaction directory under lifecycle leases.

use std::{fs, io, os::unix::fs::MetadataExt};

use super::{
    error::{FilesystemError, io_failure},
    layout::FilesystemLayout,
    paths::ensure_no_symlink_components,
    snapshot::identity,
    staging,
};

pub(super) fn cleanup(layout: &FilesystemLayout) -> Result<(), FilesystemError> {
    let path = layout.staging_root();
    ensure_no_symlink_components(path, true)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_failure("inspect stale staging", path, error)),
    };
    // Production callers already required root; fixture callers may own their
    // unprivileged tree. Apply the same effective-UID check on both Unix hosts.
    let owner = rustix::process::geteuid().as_raw();
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o7777 != 0o700 {
        return Err(FilesystemError::UnsafePath {
            path: path.to_path_buf(),
            reason: "stale staging is not a private owned transaction directory",
        });
    }
    tracing::info!(path = %path.display(), "Cleaning abandoned MixRelay transaction files");
    // The whole private directory belongs to this transaction. Recursive removal
    // unlinks nested symlinks and hardlinks without following their external targets.
    staging::remove_if_identity(path, identity(&metadata))
}
