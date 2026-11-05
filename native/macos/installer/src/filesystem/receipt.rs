//! Receipts that bind staged and committed work to one layout, request and snapshot.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::{
    directories::CreatedDirectory,
    error::FilesystemError,
    layout::{FilesystemLayout, backup_root, payload_root, staging_name},
    paths::metadata,
    snapshot::{FilesystemIdentity, FilesystemSnapshot, MetadataFingerprint, identity},
};
use crate::{
    backend::RollbackFailure,
    contract::OwnedTarget,
    mutation_journal::{self, MutationJournal},
    request::TransactionRequest,
};

pub(super) type InstalledTarget = mutation_journal::InstalledTarget<MetadataFingerprint>;
pub(super) type MutationProgress = MutationJournal<MetadataFingerprint, CreatedDirectory, PathBuf>;

/// The inputs a receipt was created for; any other combination is rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TransactionBinding {
    pub(super) layout: FilesystemLayout,
    pub(super) request: TransactionRequest,
    pub(super) snapshot: FilesystemSnapshot,
}

/// Private staged payload and mutable partial-commit receipt.
#[derive(Debug)]
pub struct FilesystemStaged {
    pub(super) staging_root: PathBuf,
    pub(super) staging_identity: FilesystemIdentity,
    pub(super) binding: TransactionBinding,
    pub(super) progress: Mutex<MutationProgress>,
}

impl FilesystemStaged {
    /// Returns the private staging directory.
    #[must_use]
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    /// Returns the private payload path for validation before commit.
    /// Removal stages do not contain payloads. This path is not a destination.
    #[must_use]
    pub fn payload_path(&self, target: OwnedTarget) -> PathBuf {
        payload_root(&self.staging_root).join(staging_name(target))
    }

    pub(crate) fn backup_path(&self, target: OwnedTarget) -> PathBuf {
        backup_root(&self.staging_root).join(staging_name(target))
    }

    /// Fails when the staging path no longer names the directory this receipt created.
    pub(super) fn ensure_identity(&self) -> Result<(), FilesystemError> {
        let observed = identity(&metadata(&self.staging_root, "private staging")?);
        if observed != self.staging_identity {
            return Err(FilesystemError::StagingIdentityChanged {
                path: self.staging_root.clone(),
            });
        }
        Ok(())
    }

    pub(super) fn rollback_failure(
        &self,
        error: FilesystemError,
    ) -> RollbackFailure<FilesystemError, FilesystemRecoveryDiagnostics> {
        let progress = self
            .progress
            .try_lock()
            .map_or_else(|_| MutationProgress::default(), |progress| progress.clone());
        self.rollback_failure_with_progress(error, &progress)
    }

    pub(super) fn rollback_failure_with_progress(
        &self,
        error: FilesystemError,
        progress: &MutationProgress,
    ) -> RollbackFailure<FilesystemError, FilesystemRecoveryDiagnostics> {
        RollbackFailure::new(
            error,
            FilesystemRecoveryDiagnostics {
                staging_root: self.staging_root.clone(),
                backed_up_targets: progress.backups().to_vec(),
                installed_targets: installed_targets(progress),
            },
        )
    }
}

/// Receipt for a completed filesystem commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemCommitted {
    pub(super) staging_identity: FilesystemIdentity,
    pub(super) binding: TransactionBinding,
    pub(super) progress: MutationProgress,
}

impl FilesystemCommitted {
    /// Returns targets moved to private backups by the commit.
    #[must_use]
    pub fn backed_up_targets(&self) -> &[OwnedTarget] {
        self.progress.backups()
    }

    /// Returns targets installed from private staging by the commit.
    #[must_use]
    pub fn installed_targets(&self) -> Vec<OwnedTarget> {
        installed_targets(&self.progress)
    }

    pub(super) fn belongs_to(&self, staged: &FilesystemStaged) -> bool {
        self.binding == staged.binding && self.staging_identity == staged.staging_identity
    }
}

fn installed_targets(progress: &MutationProgress) -> Vec<OwnedTarget> {
    progress
        .installed()
        .iter()
        .map(|installed| installed.target)
        .collect()
}

/// Filesystem rollback diagnostics captured before mandatory staging cleanup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemRecoveryDiagnostics {
    staging_root: PathBuf,
    backed_up_targets: Vec<OwnedTarget>,
    installed_targets: Vec<OwnedTarget>,
}

impl FilesystemRecoveryDiagnostics {
    /// Returns the attempted staging path; successful cleanup removes it.
    #[must_use]
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    /// Returns targets moved into private backups before the failure.
    #[must_use]
    pub fn backed_up_targets(&self) -> &[OwnedTarget] {
        &self.backed_up_targets
    }

    /// Returns targets installed before the failure.
    #[must_use]
    pub fn installed_targets(&self) -> &[OwnedTarget] {
        &self.installed_targets
    }
}
