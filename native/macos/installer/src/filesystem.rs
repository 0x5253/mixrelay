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

use std::os::unix::fs::PermissionsExt as _;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use thiserror::Error;

use crate::{
    backend::RollbackFailure,
    contract::{LIFECYCLE_DIRECTORY, OwnedTarget, PRODUCT_DIRECTORY},
    mutation_journal::{FilesystemStageState, MutationJournal, RollbackStore},
    request::TransactionRequest,
};

const STAGING_DIRECTORY_NAME: &str = ".installer-staging";
const PAYLOAD_DIRECTORY_NAME: &str = "payload";
const BACKUP_DIRECTORY_NAME: &str = "backups";
const APP_PAYLOAD_NAME: &str = "app";
const DRIVER_PAYLOAD_NAME: &str = "driver";
const HAL_PAYLOAD_NAME: &str = "hal";
const LAUNCHD_PAYLOAD_NAME: &str = "launchd";
const UNINSTALLER_PAYLOAD_NAME: &str = "uninstaller";
const REMOVAL_LAUNCHD_PAYLOAD_NAME: &str = "removal-launchd";
const RUNTIME_ENGINE_PAYLOAD_NAME: &str = "runtime-engine";
const RUNTIME_LAUNCHD_PAYLOAD_NAME: &str = "runtime-launchd";

/// The expected filesystem kind of an owned destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetKind {
    Directory,
    File,
}

/// The public presence observed for one owned destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetPresence {
    /// The destination does not exist.
    Absent,
    /// The destination exists with the expected non-symlink kind.
    Present,
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

/// A filesystem failure with enough path and operation context for recovery.
#[derive(Debug, Error)]
pub enum FilesystemError {
    /// A path was not absolute, normalized, or safe for this adapter.
    #[error("unsafe filesystem path {path}: {reason}")]
    UnsafePath {
        /// The rejected path.
        path: PathBuf,
        /// The reason the path was rejected.
        reason: &'static str,
    },
    /// An expected artifact or directory was absent.
    #[error("missing {kind}: {path}")]
    Missing { kind: &'static str, path: PathBuf },
    /// A path had an unexpected inode kind.
    #[error("unexpected filesystem kind for {kind}: {path}")]
    UnexpectedKind { kind: &'static str, path: PathBuf },
    /// A source artifact failed its shape or mode validation.
    #[error("invalid {kind} source {path}: {reason}")]
    InvalidSource {
        /// The source kind being validated.
        kind: &'static str,
        /// The rejected source path.
        path: PathBuf,
        /// The failed validation.
        reason: &'static str,
    },
    /// A target changed between read-only preflight and mutation.
    #[error("owned target changed after preflight: {target:?}")]
    ConcurrentTargetChange {
        /// The target whose fingerprint changed.
        target: OwnedTarget,
    },
    /// A prior failed transaction left its staging evidence in place.
    #[error("private staging is already occupied: {path}")]
    StagingOccupied {
        /// The retained staging path.
        path: PathBuf,
    },
    /// A stage operation failed and its private cleanup also failed.
    #[error("staging failed: {cause}; staging cleanup failed: {cleanup}")]
    StagingCleanup {
        /// The original staging failure.
        cause: Box<Self>,
        /// The cleanup failure that caused evidence to remain.
        cleanup: Box<Self>,
    },
    /// An operation on a path failed.
    #[error("filesystem {operation} failed for {path}: {source}")]
    Io {
        /// The operation attempted on the path.
        operation: &'static str,
        /// The affected path.
        path: PathBuf,
        /// The operating-system failure.
        #[source]
        source: io::Error,
    },
    /// An internal receipt was in use by another boundary.
    #[error("filesystem mutation receipt is busy")]
    ReceiptBusy,
    /// The filesystem receipt mutex was poisoned by a prior panic.
    #[error("filesystem mutation receipt was poisoned")]
    ReceiptPoisoned,
    /// A supplied commit receipt belongs to another staging directory.
    #[error("commit receipt does not belong to this staging directory")]
    ReceiptMismatch,
    /// The private staging inode was replaced after creation.
    #[error("private staging identity changed: {path}")]
    StagingIdentityChanged {
        /// The staging path that no longer names the captured inode.
        path: PathBuf,
    },
    /// A parent directory created by this transaction was replaced before rollback.
    #[error("created destination directory identity changed: {path}")]
    CreatedDirectoryChanged {
        /// The directory that no longer names the transaction-created inode.
        path: PathBuf,
    },
    /// A private staging inode could not be identified after creation.
    #[error("private staging identity unavailable; retained staging path: {path}: {cause}")]
    StagingIdentityUnavailable {
        /// The staging path that requires administrator inspection.
        path: PathBuf,
        /// The identity-read failure.
        #[source]
        cause: Box<Self>,
    },
    /// A created parent directory could not be identified for safe rollback.
    #[error("created destination directory identity unavailable; retained path: {path}: {cause}")]
    CreatedDirectoryIdentityUnavailable {
        /// The path that requires administrator inspection.
        path: PathBuf,
        /// The identity-read failure.
        #[source]
        cause: Box<Self>,
    },
    /// The receipt is not in a state that permits this operation.
    #[error("filesystem operation {operation} is invalid while receipt is {state:?}")]
    InvalidStageState {
        /// The operation that was rejected.
        operation: &'static str,
        /// The current receipt state.
        state: FilesystemStageState,
    },
}

/// An absolute root used to map the fixed production destinations into a fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemLayout {
    root: PathBuf,
    staging_root: PathBuf,
}

impl FilesystemLayout {
    /// Creates a layout rooted at `root`; `/` is the production root.
    ///
    /// The root must already exist and contain no symlink component. The
    /// lifecycle parent may be created by a privileged preflight, but this
    /// adapter will not create it on its own.
    ///
    /// # Errors
    /// Returns an unsafe-path or filesystem error when `root` is not a safe directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, FilesystemError> {
        let root = root.into();
        validate_absolute_path(&root)?;
        ensure_no_symlink_components(&root, false)?;
        require_directory(&root, "filesystem root")?;
        let lifecycle = root.join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
        let staging_root = lifecycle.join(STAGING_DIRECTORY_NAME);
        Ok(Self { root, staging_root })
    }

    /// Returns the absolute fixture or production root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the private transaction staging path.
    #[must_use]
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    pub(crate) fn target_path(&self, target: OwnedTarget) -> PathBuf {
        self.root.join(target.path().trim_start_matches('/'))
    }
}

/// Unprivileged filesystem adapter for exact owned payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemAdapter {
    layout: FilesystemLayout,
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

    pub(crate) fn snapshot_targets(&self) -> Result<FilesystemSnapshot, FilesystemError> {
        Ok(FilesystemSnapshot {
            layout: self.layout.clone(),
            targets: self.observe_targets()?,
            product_directory: self.observe_product_directory()?,
        })
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
        let staging_root = &self.layout.staging_root;
        create_private_directory(staging_root)?;
        let staging_identity = match metadata(staging_root, "private staging") {
            Ok(metadata) => identity(&metadata),
            Err(cause) => {
                return Err(FilesystemError::StagingIdentityUnavailable {
                    path: staging_root.clone(),
                    cause: Box::new(cause),
                });
            }
        };
        let result = Self::populate_staging(request, staging_root);
        match result {
            Ok(()) => Ok(FilesystemStaged {
                staging_root: staging_root.clone(),
                staging_identity,
                binding: TransactionBinding {
                    layout: self.layout.clone(),
                    request: request.clone(),
                    snapshot: snapshot.clone(),
                },
                progress: Mutex::new(MutationProgress::new()),
            }),
            Err(cause) => match remove_staging_root_if_identity(staging_root, staging_identity) {
                Ok(()) => Err(cause),
                Err(cleanup) => Err(FilesystemError::StagingCleanup {
                    cause: Box::new(cause),
                    cleanup: Box::new(cleanup),
                }),
            },
        }
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
        ensure_staging_identity(staged)?;
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
            return Err(Box::new(Self::rollback_failure(staged, error)));
        }
        let mut progress = match staged.progress.try_lock() {
            Ok(progress) => progress,
            Err(error) => {
                return Err(Box::new(Self::rollback_failure(
                    staged,
                    receipt_lock_error(&error),
                )));
            }
        };
        if progress.state() == FilesystemStageState::Cleaned {
            return Err(Box::new(Self::rollback_failure_with_progress(
                staged,
                FilesystemError::InvalidStageState {
                    operation: "rollback",
                    state: progress.state(),
                },
                &progress,
            )));
        }
        if let Err(error) = ensure_staging_identity(staged) {
            progress.rollback_failed();
            return Err(Box::new(Self::rollback_failure_with_progress(
                staged, error, &progress,
            )));
        }
        if let Some(receipt) = committed
            && (receipt.binding != staged.binding
                || receipt.staging_identity != staged.staging_identity)
        {
            return Err(Box::new(Self::rollback_failure_with_progress(
                staged,
                FilesystemError::ReceiptMismatch,
                &progress,
            )));
        }
        let result = self.restore_progress(snapshot, staged, &mut progress);
        if let Err(error) = result {
            progress.rollback_failed();
            return Err(Box::new(Self::rollback_failure_with_progress(
                staged, error, &progress,
            )));
        }
        if let Err(error) = self.ensure_snapshot(snapshot) {
            progress.rollback_failed();
            return Err(Box::new(Self::rollback_failure_with_progress(
                staged, error, &progress,
            )));
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
        if progress.state() == FilesystemStageState::Committed
            && staged.binding.request.kind() == crate::OperationKind::Remove
        {
            self.remove_empty_product_directory(staged.binding.snapshot.product_directory)?;
        }
        remove_staging_root_if_identity(&staged.staging_root, staged.staging_identity)?;
        progress.cleaned();
        Ok(())
    }

    fn validate_staging_location(&self) -> Result<(), FilesystemError> {
        let lifecycle =
            self.layout
                .staging_root
                .parent()
                .ok_or_else(|| FilesystemError::UnsafePath {
                    path: self.layout.staging_root.clone(),
                    reason: "staging path has no lifecycle parent",
                })?;
        ensure_no_symlink_components(lifecycle, false)?;
        require_directory(lifecycle, "lifecycle directory")?;
        ensure_no_symlink_components(&self.layout.staging_root, true)?;
        match fs::symlink_metadata(&self.layout.staging_root) {
            Ok(_) => Err(FilesystemError::StagingOccupied {
                path: self.layout.staging_root.clone(),
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_failure("inspect", &self.layout.staging_root, error)),
        }
    }

    fn observe_targets(&self) -> Result<[TargetSnapshot; 8], FilesystemError> {
        let targets = OwnedTarget::all();
        Ok([
            self.observe_target(targets[0])?,
            self.observe_target(targets[1])?,
            self.observe_target(targets[2])?,
            self.observe_target(targets[3])?,
            self.observe_target(targets[4])?,
            self.observe_target(targets[5])?,
            self.observe_target(targets[6])?,
            self.observe_target(targets[7])?,
        ])
    }

    fn observe_target(&self, target: OwnedTarget) -> Result<TargetSnapshot, FilesystemError> {
        let path = self.layout.target_path(target);
        ensure_no_symlink_components(&path, true)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(FilesystemError::UnsafePath {
                        path,
                        reason: "owned target is a symlink",
                    });
                }
                let kind = target_kind(target);
                if !has_expected_kind(&metadata, kind) {
                    return Err(FilesystemError::UnexpectedKind {
                        kind: target_name(target),
                        path,
                    });
                }
                Ok(TargetSnapshot {
                    target,
                    presence: SnapshotPresence::Present(fingerprint(&metadata)),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(TargetSnapshot {
                target,
                presence: SnapshotPresence::Absent,
            }),
            Err(error) => Err(io_failure("inspect", &path, error)),
        }
    }

    pub(crate) fn ensure_snapshot(
        &self,
        snapshot: &FilesystemSnapshot,
    ) -> Result<(), FilesystemError> {
        if snapshot.layout != self.layout {
            return Err(FilesystemError::ReceiptMismatch);
        }
        if self.observe_product_directory()? != snapshot.product_directory {
            return Err(FilesystemError::CreatedDirectoryChanged {
                path: self.product_directory_path(),
            });
        }
        for target in OwnedTarget::all() {
            let observed = self.observe_target(target)?;
            let expected = snapshot.target(target);
            if &observed != expected {
                return Err(FilesystemError::ConcurrentTargetChange { target });
            }
        }
        Ok(())
    }

    fn ensure_staged_binding(
        &self,
        request: &TransactionRequest,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
    ) -> Result<(), FilesystemError> {
        let expected = TransactionBinding {
            layout: self.layout.clone(),
            request: request.clone(),
            snapshot: snapshot.clone(),
        };
        if staged.binding == expected {
            Ok(())
        } else {
            Err(FilesystemError::ReceiptMismatch)
        }
    }

    fn validate_sources(&self, sources: &crate::InstallSources) -> Result<(), FilesystemError> {
        let paths = [
            ("app", sources.app(), TargetKind::Directory),
            ("hal", sources.hal(), TargetKind::Directory),
            ("driver", sources.driver(), TargetKind::File),
            ("launchd", sources.launchd(), TargetKind::File),
            (
                UNINSTALLER_PAYLOAD_NAME,
                sources.remover(),
                TargetKind::Directory,
            ),
            (
                REMOVAL_LAUNCHD_PAYLOAD_NAME,
                sources.removal_launchd(),
                TargetKind::File,
            ),
            (
                RUNTIME_ENGINE_PAYLOAD_NAME,
                sources.runtime_engine(),
                TargetKind::File,
            ),
            (
                RUNTIME_LAUNCHD_PAYLOAD_NAME,
                sources.runtime_launchd(),
                TargetKind::File,
            ),
        ];
        for (index, (_, path, _)) in paths.iter().enumerate() {
            for (_, other, _) in paths.iter().skip(index + 1) {
                if is_within(path, other) || is_within(other, path) {
                    return Err(FilesystemError::UnsafePath {
                        path: path.to_path_buf(),
                        reason: "source artifacts overlap",
                    });
                }
            }
        }
        for (name, path, kind) in paths {
            validate_absolute_path(path)?;
            ensure_no_symlink_components(path, false)?;
            let metadata = metadata(path, name)?;
            if !has_expected_kind(&metadata, kind) {
                return Err(FilesystemError::InvalidSource {
                    kind: name,
                    path: path.to_path_buf(),
                    reason: "unexpected inode kind",
                });
            }
            if kind == TargetKind::File {
                validate_file_source(name, path, &metadata)?;
            } else {
                validate_bundle_source(name, path)?;
            }
            self.ensure_source_disjoint(path)?;
        }
        Ok(())
    }

    fn ensure_source_disjoint(&self, source: &Path) -> Result<(), FilesystemError> {
        if is_within(source, &self.layout.staging_root)
            || is_within(&self.layout.staging_root, source)
            || OwnedTarget::all().iter().any(|target| {
                let destination = self.layout.target_path(*target);
                is_within(source, &destination) || is_within(&destination, source)
            })
        {
            return Err(FilesystemError::UnsafePath {
                path: source.to_path_buf(),
                reason: "source overlaps an owned destination or private staging",
            });
        }
        Ok(())
    }

    fn populate_staging(
        request: &TransactionRequest,
        staging_root: &Path,
    ) -> Result<(), FilesystemError> {
        let payload_root = staging_root.join(PAYLOAD_DIRECTORY_NAME);
        let backup_root = staging_root.join(BACKUP_DIRECTORY_NAME);
        create_directory(&payload_root)?;
        create_directory(&backup_root)?;
        if let TransactionRequest::Install(sources) = request {
            let (engine, plist) = (sources.runtime_engine(), sources.runtime_launchd());
            copy_tree(engine, &payload_root.join(RUNTIME_ENGINE_PAYLOAD_NAME))?;
            copy_tree(plist, &payload_root.join(RUNTIME_LAUNCHD_PAYLOAD_NAME))?;
            fs::set_permissions(
                payload_root.join(RUNTIME_ENGINE_PAYLOAD_NAME),
                fs::Permissions::from_mode(0o755),
            )
            .map_err(|error| io_failure("set runtime engine mode", engine, error))?;
            fs::set_permissions(
                payload_root.join(RUNTIME_LAUNCHD_PAYLOAD_NAME),
                fs::Permissions::from_mode(0o644),
            )
            .map_err(|error| io_failure("set runtime launchd mode", plist, error))?;
            copy_tree(sources.app(), &payload_root.join(APP_PAYLOAD_NAME))?;
            copy_tree(sources.driver(), &payload_root.join(DRIVER_PAYLOAD_NAME))?;
            copy_tree(sources.hal(), &payload_root.join(HAL_PAYLOAD_NAME))?;
            copy_tree(sources.launchd(), &payload_root.join(LAUNCHD_PAYLOAD_NAME))?;
            let (app, plist) = (sources.remover(), sources.removal_launchd());
            copy_tree(app, &payload_root.join(UNINSTALLER_PAYLOAD_NAME))?;
            copy_tree(plist, &payload_root.join(REMOVAL_LAUNCHD_PAYLOAD_NAME))?;
            fs::set_permissions(
                payload_root.join(REMOVAL_LAUNCHD_PAYLOAD_NAME),
                std::os::unix::fs::PermissionsExt::from_mode(0o644),
            )
            .map_err(|error| io_failure("set removal launchd permissions", plist, error))?;
            // Launch configuration is data even when its source had executable bits.
            fs::set_permissions(
                payload_root.join(LAUNCHD_PAYLOAD_NAME),
                std::os::unix::fs::PermissionsExt::from_mode(0o644),
            )
            .map_err(|error| {
                io_failure(
                    "set launchd permissions",
                    &payload_root.join(LAUNCHD_PAYLOAD_NAME),
                    error,
                )
            })?;
        }
        Ok(())
    }

    fn commit_target(
        &self,
        context: &CommitContext<'_>,
        target: OwnedTarget,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        let observed = self.observe_target(target)?;
        if &observed != context.snapshot.target(target) {
            return Err(FilesystemError::ConcurrentTargetChange { target });
        }
        let destination = self.layout.target_path(target);
        if observed.presence.is_present() || context.request.kind() == crate::OperationKind::Install
        {
            Self::ensure_destination_parent(&destination, progress)?;
        }
        let product = if matches!(target, OwnedTarget::App | OwnedTarget::Uninstaller) {
            self.open_product_directory(context.snapshot, progress)?
        } else {
            None
        };
        self.commit_target_at(context, target, product.as_ref(), progress)
    }

    fn commit_target_at(
        &self,
        context: &CommitContext<'_>,
        target: OwnedTarget,
        product: Option<&fs::File>,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        if context.snapshot.target(target).presence.is_present() {
            let backup = context.staged.backup_path(target);
            ensure_absent(&backup)?;
            self.move_target_to_private(product, target, &backup)?;
            progress.backed_up(target);
        }
        if context.request.kind() == crate::OperationKind::Install {
            let payload = context.staged.payload_path(target);
            validate_staged_payload(target, &payload)?;
            let payload_fingerprint = fingerprint(&metadata(&payload, "staged payload")?);
            self.move_private_to_target(product, target, &payload)?;
            progress.installed_target(InstalledTarget {
                target,
                fingerprint: payload_fingerprint,
            });
        }
        Ok(())
    }

    fn ensure_destination_parent(
        destination: &Path,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        let parent = destination
            .parent()
            .ok_or_else(|| FilesystemError::UnsafePath {
                path: destination.to_path_buf(),
                reason: "destination has no parent",
            })?;
        let (created, unidentified) = progress.directory_progress();
        create_missing_directories(parent, created, unidentified)
    }

    fn restore_progress(
        &self,
        snapshot: &FilesystemSnapshot,
        staged: &FilesystemStaged,
        progress: &mut MutationProgress,
    ) -> Result<(), FilesystemError> {
        let product = self.open_product_directory(snapshot, progress)?;
        self.restore_progress_at(snapshot, staged, progress, product.as_ref())
    }

    fn restore_progress_at(
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

    fn rollback_failure(
        staged: &FilesystemStaged,
        error: FilesystemError,
    ) -> RollbackFailure<FilesystemError, FilesystemRecoveryDiagnostics> {
        let progress = staged
            .progress
            .try_lock()
            .map_or_else(|_| MutationProgress::default(), |progress| progress.clone());
        Self::rollback_failure_with_progress(staged, error, &progress)
    }

    fn rollback_failure_with_progress(
        staged: &FilesystemStaged,
        error: FilesystemError,
        progress: &MutationProgress,
    ) -> RollbackFailure<FilesystemError, FilesystemRecoveryDiagnostics> {
        RollbackFailure::new(
            error,
            FilesystemRecoveryDiagnostics {
                staging_root: staged.staging_root.clone(),
                backed_up_targets: progress.backups().to_vec(),
                installed_targets: progress
                    .installed()
                    .iter()
                    .map(|installed| installed.target)
                    .collect(),
            },
        )
    }
}

/// The exact target state captured by filesystem preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemSnapshot {
    layout: FilesystemLayout,
    targets: [TargetSnapshot; 8],
    product_directory: Option<FilesystemIdentity>,
}

impl FilesystemSnapshot {
    /// Returns whether one exact destination was present during preflight.
    #[must_use]
    pub fn presence(&self, target: OwnedTarget) -> TargetPresence {
        self.target(target).presence.public()
    }

    fn target(&self, target: OwnedTarget) -> &TargetSnapshot {
        match self
            .targets
            .iter()
            .find(|snapshot| snapshot.target == target)
        {
            Some(snapshot) => snapshot,
            None => unreachable!("OwnedTarget::all always contains every target"),
        }
    }
}

/// Private staged payload and mutable partial-commit receipt.
#[derive(Debug)]
pub struct FilesystemStaged {
    staging_root: PathBuf,
    staging_identity: FilesystemIdentity,
    binding: TransactionBinding,
    progress: Mutex<MutationProgress>,
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
        self.staging_root
            .join(PAYLOAD_DIRECTORY_NAME)
            .join(target_name(target))
    }

    pub(crate) fn backup_path(&self, target: OwnedTarget) -> PathBuf {
        self.staging_root
            .join(BACKUP_DIRECTORY_NAME)
            .join(target_name(target))
    }
}

/// Receipt for a completed filesystem commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemCommitted {
    staging_identity: FilesystemIdentity,
    binding: TransactionBinding,
    progress: MutationProgress,
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
        self.progress
            .installed()
            .iter()
            .map(|installed| installed.target)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MetadataFingerprint {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanos: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SnapshotPresence {
    Absent,
    Present(MetadataFingerprint),
}

impl SnapshotPresence {
    fn is_present(self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn public(self) -> TargetPresence {
        match self {
            Self::Absent => TargetPresence::Absent,
            Self::Present(_) => TargetPresence::Present,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TargetSnapshot {
    target: OwnedTarget,
    presence: SnapshotPresence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TransactionBinding {
    layout: FilesystemLayout,
    request: TransactionRequest,
    snapshot: FilesystemSnapshot,
}

struct CommitContext<'a> {
    request: &'a TransactionRequest,
    snapshot: &'a FilesystemSnapshot,
    staged: &'a FilesystemStaged,
}

type InstalledTarget = crate::mutation_journal::InstalledTarget<MetadataFingerprint>;
type MutationProgress = MutationJournal<MetadataFingerprint, CreatedDirectory, PathBuf>;

struct FilesystemRollback<'a> {
    adapter: &'a FilesystemAdapter,
    staged: &'a FilesystemStaged,
    product: Option<&'a fs::File>,
}

impl RollbackStore<MetadataFingerprint, CreatedDirectory, PathBuf> for FilesystemRollback<'_> {
    type Error = FilesystemError;
    fn remove_installed(&mut self, installed: &InstalledTarget) -> Result<(), FilesystemError> {
        if matches!(
            installed.target,
            OwnedTarget::App | OwnedTarget::Uninstaller
        ) {
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
        if !matches!(target, OwnedTarget::App | OwnedTarget::Uninstaller) {
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct CreatedDirectory {
    path: PathBuf,
    identity: FilesystemIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FilesystemIdentity {
    device: u64,
    inode: u64,
}

mod payload;
mod product;
mod stale;
#[cfg(test)]
mod tests;

use payload::{
    copy_tree, create_directory, create_missing_directories, create_private_directory,
    ensure_absent, ensure_no_symlink_components, ensure_staging_identity, fingerprint,
    has_expected_kind, identity, io_failure, is_within, metadata, receipt_lock_error,
    remove_created_directory, remove_installed_path, remove_staging_root_if_identity, rename_path,
    require_directory, target_kind, target_name, validate_absolute_path, validate_bundle_source,
    validate_file_source, validate_staged_payload,
};
