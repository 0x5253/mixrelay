//! The private staging tree: copied payloads, backup slots and identity-checked removal.

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use super::{
    directories::create_directory,
    error::{FilesystemError, io_failure},
    layout::{TargetKind, backup_root, payload_root, staging_name},
    paths::{ensure_no_symlink_components, metadata, metadata_if_present},
    snapshot::{FilesystemIdentity, identity},
    sources::source_path,
    tree::{copy_tree, validate_tree},
};
use crate::{contract::OwnedTarget, request::TransactionRequest};

/// Copy order, and the mode each payload gets regardless of its source mode.
/// Launch configuration is data even when its source had executable bits.
const STAGED_PAYLOADS: [(OwnedTarget, Option<u32>); 8] = [
    (OwnedTarget::RuntimeEngine, Some(0o755)),
    (OwnedTarget::RuntimeLaunchd, Some(0o644)),
    (OwnedTarget::App, None),
    (OwnedTarget::Driver, None),
    (OwnedTarget::Hal, None),
    (OwnedTarget::Launchd, Some(0o644)),
    (OwnedTarget::Uninstaller, None),
    (OwnedTarget::RemovalLaunchd, Some(0o644)),
];

/// Creates the payload and backup directories and copies an install's sources.
pub(super) fn populate(
    request: &TransactionRequest,
    staging_root: &Path,
) -> Result<(), FilesystemError> {
    let payloads = payload_root(staging_root);
    create_directory(&payloads)?;
    create_directory(&backup_root(staging_root))?;
    let TransactionRequest::Install(sources) = request else {
        return Ok(());
    };
    for (target, mode) in STAGED_PAYLOADS {
        let payload = payloads.join(staging_name(target));
        copy_tree(source_path(sources, target), &payload)?;
        if let Some(mode) = mode {
            fs::set_permissions(&payload, fs::Permissions::from_mode(mode))
                .map_err(|error| io_failure("set staged payload mode", &payload, error))?;
        }
    }
    Ok(())
}

/// A staged payload must still be the expected kind with a symlink-free tree.
pub(super) fn validate_payload(target: OwnedTarget, path: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, false)?;
    let metadata = metadata(path, "staged payload")?;
    if !TargetKind::of(target).matches(&metadata) {
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

/// Removes staging only while it is still the directory this transaction created.
pub(super) fn remove_if_identity(
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
    remove(path)
}

fn remove(path: &Path) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    match metadata_if_present(path, "inspect")? {
        Some(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|error| io_failure("remove staging", path, error))
        }
        Some(_) => Err(FilesystemError::UnexpectedKind {
            kind: "private staging directory",
            path: path.to_path_buf(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_is_staged_exactly_once() {
        for target in OwnedTarget::all() {
            let entries = STAGED_PAYLOADS
                .iter()
                .filter(|(staged, _)| *staged == target)
                .count();
            assert_eq!(entries, 1, "{target:?}");
        }
    }
}
