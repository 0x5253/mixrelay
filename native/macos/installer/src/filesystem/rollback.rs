//! Undoes a partial commit through identity-checked moves; user data is never a target.

use std::{fs, path::Path, path::PathBuf};

use super::{
    FilesystemAdapter,
    directories::{CreatedDirectory, remove_created_directory},
    error::{FilesystemError, io_failure},
    layout::{TargetKind, in_product_directory},
    paths::{ensure_absent, ensure_no_symlink_components, metadata, metadata_if_present},
    receipt::{FilesystemStaged, InstalledTarget, MutationProgress},
    snapshot::{FilesystemSnapshot, MetadataFingerprint, fingerprint},
};
use crate::{contract::OwnedTarget, mutation_journal::RollbackStore};

impl FilesystemAdapter {
    pub(super) fn restore_progress(
        &self,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        let product = self.open_product_directory(snapshot, progress)?;
        self.restore_progress_at(snapshot, staged, progress, product.as_ref())
    }

    /// Replays the journal in reverse, then requires the preflight snapshot again.
    pub(super) fn restore_progress_at(
        &self,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
        progress: &mut MutationProgress,
        product: Option<&fs::File>,
    ) -> Result<(), FilesystemError> {
        progress.restore(&mut FilesystemRollback {
            adapter: self,
            staged,
            product,
        })?;
        self.ensure_snapshot(snapshot)
    }
}

struct FilesystemRollback<'a> {
    adapter: &'a FilesystemAdapter,
    staged: &'a FilesystemStaged,
    product: Option<&'a fs::File>,
}

impl RollbackStore<MetadataFingerprint, CreatedDirectory, PathBuf> for FilesystemRollback<'_> {
    type Error = FilesystemError;

    fn remove_installed(&mut self, installed: &InstalledTarget) -> Result<(), FilesystemError> {
        if in_product_directory(installed.target) {
            self.adapter
                .rollback_installed_product(self.product, self.staged, installed)
        } else {
            remove_installed_path(
                &self.adapter.layout.target_path(installed.target),
                installed,
            )
        }
    }

    fn restore_backup(&mut self, target: OwnedTarget) -> Result<(), FilesystemError> {
        if !in_product_directory(target) {
            ensure_absent(&self.adapter.layout.target_path(target))?;
        }
        self.adapter
            .move_private_to_target(self.product, target, &self.staged.backup_path(target))
    }

    fn unidentified_directory(&self, path: &PathBuf) -> FilesystemError {
        FilesystemError::CreatedDirectoryIdentityUnavailable {
            path: path.clone(),
            cause: Box::new(FilesystemError::Missing {
                kind: "created destination directory metadata",
                path: path.clone(),
            }),
        }
    }

    fn remove_directory(&mut self, directory: &CreatedDirectory) -> Result<(), FilesystemError> {
        if directory.path == self.adapter.product_directory_path() {
            self.adapter
                .remove_empty_product_directory(Some(directory.identity))
        } else {
            remove_created_directory(directory)
        }
    }
}

/// Removes an installed target only while it is still the inode this commit installed.
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

fn remove_owned_path(path: &Path, target: OwnedTarget) -> Result<(), FilesystemError> {
    ensure_no_symlink_components(path, true)?;
    let Some(metadata) = metadata_if_present(path, "inspect")? else {
        return Ok(());
    };
    if !TargetKind::of(target).matches(&metadata) {
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
