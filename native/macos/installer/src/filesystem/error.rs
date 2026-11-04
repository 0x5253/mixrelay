//! Typed filesystem failures with the path and operation needed for recovery.

use std::{io, path::Path, path::PathBuf, sync::TryLockError};

use thiserror::Error;

use crate::{contract::OwnedTarget, mutation_journal::FilesystemStageState};

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

pub(super) fn io_failure(
    operation: &'static str,
    path: &Path,
    source: io::Error,
) -> FilesystemError {
    FilesystemError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

pub(super) fn receipt_lock_error<T>(error: &TryLockError<T>) -> FilesystemError {
    match error {
        TryLockError::WouldBlock => FilesystemError::ReceiptBusy,
        TryLockError::Poisoned(_) => FilesystemError::ReceiptPoisoned,
    }
}
