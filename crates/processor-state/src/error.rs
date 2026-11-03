use std::io;

use thiserror::Error;

/// Failure while validating, storing, or transferring processor state.
#[derive(Debug, Error)]
pub enum StateError {
    /// A digest or declared state size is invalid.
    #[error("invalid processor state reference: {reason}")]
    InvalidReference { reason: &'static str },
    /// A bundle violates its bounded binary framing contract.
    #[error("invalid processor state bundle: {reason}")]
    InvalidBundle { reason: &'static str },
    /// The store path is not a safe private directory owned by this process user.
    #[error("processor state store directory is unsafe")]
    UnsafeStoreDirectory,
    /// A blob path resolves to a symlink, non-regular file, wrong owner, or wrong mode.
    #[error("processor state store entry is unsafe")]
    UnsafeStoreEntry,
    /// No blob exists for the validated reference.
    #[error("processor state blob is missing")]
    MissingBlob,
    /// Stored or bundled payload bytes do not match their declared reference.
    #[error("processor state payload integrity check failed")]
    Integrity,
    /// A bounded allocation could not be reserved.
    #[error("processor state allocation failed")]
    Allocation,
    /// Filesystem or stream I/O failed.
    #[error("processor state I/O failed: {0}")]
    Io(#[from] io::Error),
}
