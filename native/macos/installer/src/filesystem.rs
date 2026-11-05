//! Unprivileged filesystem work for the macOS distribution transaction.
//!
//! This module deliberately stops at files and directories. It does not acquire
//! lifecycle leases, drain a running owner, call `launchctl`, reload `CoreAudio`,
//! change ownership, or verify signatures. The native privileged adapter can
//! compose these operations with the pure transaction core.
//!
//! App mutations retain the validated product-directory descriptor across renames.
//! Other destinations and private staging require protected parent directories
//! and lifecycle leases held by the privileged native adapter.
//!
//! Removal and cleanup are limited to the eight `OwnedTarget` paths, private
//! staging, and the validated empty product folder. Lifecycle lease inodes and
//! user data are never recursive removal targets here.

mod directories;
pub(crate) mod error;
pub(crate) mod layout;
mod paths;
mod plan;
mod product;
pub(crate) mod receipt;
mod rollback;
pub(crate) mod snapshot;
mod sources;
mod staging;
mod stale;
#[cfg(test)]
mod tests;
mod tree;

use std::sync::Mutex;

use crate::{
    backend::RollbackFailure, contract::OwnedTarget, mutation_journal::FilesystemStageState,
    request::TransactionRequest,
};
use directories::{create_missing_directories, create_private_directory};
use error::{FilesystemError, receipt_lock_error};
use layout::{FilesystemLayout, in_product_directory};
use paths::{ensure_absent, ensure_no_symlink_components, metadata, metadata_if_present};
use plan::{TargetSteps, cleans_product_directory};
use receipt::{
    FilesystemCommitted, FilesystemRecoveryDiagnostics, FilesystemStaged, InstalledTarget,
    MutationProgress, TransactionBinding,
};
use snapshot::{FilesystemSnapshot, fingerprint, identity};

/// Unprivileged filesystem adapter for exact owned payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemAdapter {
    layout: FilesystemLayout,
}

/// The inputs one commit step needs besides the target itself.
struct CommitContext<'a> {
    request: &'a TransactionRequest,
    snapshot: &'a FilesystemSnapshot,
    staged: &'a FilesystemStaged,
}

impl FilesystemAdapter {
    /// Creates an adapter for a fixture root or the privileged production root.
    #[must_use]
    pub fn new(layout: FilesystemLayout) -> Self {
        Self { layout }
    }

    /// Returns this adapter's fixed filesystem layout.
    #[must_use]
    pub fn layout(&self) -> &FilesystemLayout {
        &self.layout
    }

    /// Cleans an abandoned, caller-owned private transaction directory.
    ///
    /// The coordinated caller must hold all lifecycle leases and validate the
    /// fixed destination parents first. Abandoned original backups and unused
    /// payloads are removed for both installation and removal.
    /// # Errors
    /// Returns unsafe staging contents or cleanup errors.
    pub fn cleanup_stale_staging(&self) -> Result<(), FilesystemError> {
        stale::cleanup(&self.layout)
    }

    /// Reads exact owned target state and validates install sources without mutation.
    ///
    /// This method does not acquire lifecycle locks. A native platform adapter
    /// must hold the shared app/engine locks and exclusive installer lock before
    /// handing the snapshot to the transaction core.
    ///
    /// # Errors
    /// Returns source, target, path, staging, or filesystem validation errors.
    pub fn preflight(
        &self,
        request: &TransactionRequest,
    ) -> Result<FilesystemSnapshot, FilesystemError> {
        self.validate_staging_location()?;
        let snapshot = self.snapshot_targets()?;
        if let TransactionRequest::Install(sources) = request {
            self.validate_sources(sources)?;
        }
        Ok(snapshot)
    }

    /// Copies validated install sources into private staging without touching destinations.
    ///
    /// # Errors
    /// Returns a staging or filesystem error. A failed copy attempts to remove
    /// only its private staging directory; if that cleanup fails, both failures
    /// are returned and the staging evidence remains for recovery. If the
    /// staging identity cannot be read immediately after creation, the typed
    /// error includes the retained staging path and no cleanup is attempted.
    pub fn stage(
        &self,
        request: &TransactionRequest,
        snapshot: &FilesystemSnapshot,
    ) -> Result<FilesystemStaged, FilesystemError> {
        self.ensure_snapshot(snapshot)?;
        if let TransactionRequest::Install(sources) = request {
            self.validate_sources(sources)?;
        }
        self.validate_staging_location()?;
        let staging_root = self.layout.staging_root();
        create_private_directory(staging_root)?;
        let staging_identity = metadata(staging_root, "private staging")
            .map(|metadata| identity(&metadata))
            .map_err(|cause| FilesystemError::StagingIdentityUnavailable {
                path: staging_root.to_path_buf(),
                cause: Box::new(cause),
            })?;
        if let Err(cause) = staging::populate(request, staging_root) {
            return Err(
                match staging::remove_if_identity(staging_root, staging_identity) {
                    Ok(()) => cause,
                    Err(cleanup) => FilesystemError::StagingCleanup {
                        cause: Box::new(cause),
                        cleanup: Box::new(cleanup),
                    },
                },
            );
        }
        Ok(FilesystemStaged {
            staging_root: staging_root.to_path_buf(),
            staging_identity,
            binding: self.binding(request, snapshot),
            progress: Mutex::new(MutationProgress::new()),
        })
    }

    /// Replaces or removes exactly the eight owned destinations with recoverable renames.
    ///
    /// The operation is coordinated but not one filesystem-wide atomic operation.
    /// Partial progress remains recorded in `staged` for rollback. A failed
    /// commit marks the receipt as requiring rollback and cannot be cleaned up.
    ///
    /// # Errors
    /// Returns a filesystem or concurrent-target error; rollback must then be attempted.
    pub fn commit(
        &self,
        request: &TransactionRequest,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
    ) -> Result<FilesystemCommitted, FilesystemError> {
        self.ensure_staged_binding(request, snapshot, staged)?;
        staged.ensure_identity()?;
        self.ensure_snapshot(snapshot)?;
        let mut progress = staged
            .progress
            .try_lock()
            .map_err(|error| receipt_lock_error(&error))?;
        if progress.state() != FilesystemStageState::Uncommitted {
            return Err(FilesystemError::InvalidStageState {
                operation: "commit",
                state: progress.state(),
            });
        }
        let context = CommitContext {
            request,
            snapshot,
            staged,
        };
        for target in OwnedTarget::all() {
            if let Err(error) = self.commit_target(&context, target, &mut progress) {
                progress.commit_failed();
                return Err(error);
            }
        }
        progress.committed();
        Ok(FilesystemCommitted {
            staging_identity: staged.staging_identity,
            binding: staged.binding.clone(),
            progress: progress.clone(),
        })
    }

    /// Restores every target changed by a partial commit without deleting user data.
    ///
    /// A failed rollback returns recovery diagnostics. The coordinator must
    /// still call `cleanup` after this attempt, regardless of its result.
    ///
    /// # Errors
    /// Returns a rollback failure with filesystem recovery diagnostics.
    pub fn rollback(
        &self,
        request: &TransactionRequest,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
        committed: Option<&FilesystemCommitted>,
    ) -> Result<(), Box<RollbackFailure<FilesystemError, FilesystemRecoveryDiagnostics>>> {
        if let Err(error) = self.ensure_staged_binding(request, snapshot, staged) {
            return Err(Box::new(staged.rollback_failure(error)));
        }
        let mut progress = match staged.progress.try_lock() {
            Ok(progress) => progress,
            Err(error) => {
                return Err(Box::new(
                    staged.rollback_failure(receipt_lock_error(&error)),
                ));
            }
        };
        let fail = |error, progress: &MutationProgress| {
            Box::new(staged.rollback_failure_with_progress(error, progress))
        };
        if progress.state() == FilesystemStageState::Cleaned {
            let error = FilesystemError::InvalidStageState {
                operation: "rollback",
                state: progress.state(),
            };
            return Err(fail(error, &progress));
        }
        if let Err(error) = staged.ensure_identity() {
            progress.rollback_failed();
            return Err(fail(error, &progress));
        }
        if committed.is_some_and(|receipt| !receipt.belongs_to(staged)) {
            return Err(fail(FilesystemError::ReceiptMismatch, &progress));
        }
        let restored = self
            .restore_progress(snapshot, staged, &mut progress)
            .and_then(|()| self.ensure_snapshot(snapshot));
        if let Err(error) = restored {
            progress.rollback_failed();
            return Err(fail(error, &progress));
        }
        progress.rolled_back();
        Ok(())
    }

    /// Cleans private staging and the empty product folder after committed removal.
    ///
    /// The caller must complete service registration after a successful commit
    /// before invoking this method. On failure, attempt rollback first; cleanup
    /// also discards backups when rollback itself fails. Product-folder cleanup
    /// follows successful service registration and never runs after rollback.
    ///
    /// # Errors
    /// Returns a filesystem or folder-identity error. User files are never removed.
    pub fn cleanup(&self, staged: &FilesystemStaged) -> Result<(), FilesystemError> {
        if staged.binding.layout != self.layout {
            return Err(FilesystemError::ReceiptMismatch);
        }
        let mut progress = staged
            .progress
            .try_lock()
            .map_err(|error| receipt_lock_error(&error))?;
        if cleans_product_directory(progress.state(), staged.binding.request.kind()) {
            self.remove_empty_product_directory(staged.binding.snapshot.product_directory)?;
        }
        staging::remove_if_identity(&staged.staging_root, staged.staging_identity)?;
        progress.cleaned();
        Ok(())
    }

    fn binding(
        &self,
        request: &TransactionRequest,
        snapshot: &FilesystemSnapshot,
    ) -> TransactionBinding {
        TransactionBinding {
            layout: self.layout.clone(),
            request: request.clone(),
            snapshot: snapshot.clone(),
        }
    }

    fn ensure_staged_binding(
        &self,
        request: &TransactionRequest,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
    ) -> Result<(), FilesystemError> {
        if staged.binding == self.binding(request, snapshot) {
            Ok(())
        } else {
            Err(FilesystemError::ReceiptMismatch)
        }
    }

    /// Staging must sit, still unused, in a real lifecycle directory.
    fn validate_staging_location(&self) -> Result<(), FilesystemError> {
        let staging_root = self.layout.staging_root();
        let lifecycle = staging_root
            .parent()
            .ok_or_else(|| FilesystemError::UnsafePath {
                path: staging_root.to_path_buf(),
                reason: "staging path has no lifecycle parent",
            })?;
        ensure_no_symlink_components(lifecycle, false)?;
        paths::require_directory(lifecycle, "lifecycle directory")?;
        ensure_no_symlink_components(staging_root, true)?;
        match metadata_if_present(staging_root, "inspect")? {
            Some(_) => Err(FilesystemError::StagingOccupied {
                path: staging_root.to_path_buf(),
            }),
            None => Ok(()),
        }
    }

    fn commit_target(
        &self,
        context: &CommitContext<'_>,
        target: OwnedTarget,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        let observed = self.ensure_unchanged(context.snapshot, target)?;
        let steps = TargetSteps::plan(context.request.kind(), observed.presence.is_present());
        if steps.prepare_parent {
            let destination = self.layout.target_path(target);
            let parent = destination
                .parent()
                .ok_or_else(|| FilesystemError::UnsafePath {
                    path: destination.clone(),
                    reason: "destination has no parent",
                })?;
            let (created, unidentified) = progress.directory_progress();
            create_missing_directories(parent, created, unidentified)?;
        }
        let product = if in_product_directory(target) {
            self.open_product_directory(context.snapshot, progress)?
        } else {
            None
        };
        self.commit_target_at(context, target, product.as_ref(), progress)
    }

    /// Backs up, then installs, recording each finished move for rollback.
    fn commit_target_at(
        &self,
        context: &CommitContext<'_>,
        target: OwnedTarget,
        product: Option<&std::fs::File>,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        let present = context.snapshot.target(target).presence.is_present();
        let steps = TargetSteps::plan(context.request.kind(), present);
        if steps.back_up {
            let backup = context.staged.backup_path(target);
            ensure_absent(&backup)?;
            self.move_target_to_private(product, target, &backup)?;
            progress.backed_up(target);
        }
        if steps.install {
            let payload = context.staged.payload_path(target);
            staging::validate_payload(target, &payload)?;
            let payload_fingerprint = fingerprint(&metadata(&payload, "staged payload")?);
            self.move_private_to_target(product, target, &payload)?;
            progress.installed_target(InstalledTarget {
                target,
                fingerprint: payload_fingerprint,
            });
        }
        Ok(())
    }
}
