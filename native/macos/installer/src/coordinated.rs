//! Connects real filesystem and lease owners to the platform service boundary.

use std::{error::Error, path::PathBuf};

use thiserror::Error;

use crate::{
    FilesystemAdapter, FilesystemCommitted, FilesystemError, FilesystemLayout,
    FilesystemRecoveryDiagnostics, FilesystemSnapshot, FilesystemStaged, LIFECYCLE_DIRECTORY,
    LeaseError, LifecycleDirectory, LifecycleLeaseGuard, PlatformBackend, Preflight,
    PreflightError, PreflightResult, RollbackFailure, TransactionRequest,
};

/// Platform validation and service registration participating in file rollback.
///
/// Implementations must verify source signatures, destination ownership and the
/// selected service user before mutation. Parent directories must exclude other
/// writers throughout the transaction. The filesystem adapter alone does not
/// establish those privileged trust conditions.
pub trait ServiceLifecycle {
    /// Prior registration needed to restore the exact service state.
    type Snapshot;
    /// Successful registration/removal receipt.
    type Registration;
    /// Native validation or service error.
    type Error: Error + Send + Sync + 'static;

    /// Validates trust and captures registration without changing service state.
    /// # Errors
    /// Returns failed signature, identity, ownership or registration inspection.
    fn inspect(
        &mut self,
        request: &TransactionRequest,
        layout: &FilesystemLayout,
    ) -> Result<Self::Snapshot, Self::Error>;

    /// Verifies the actual private copies before any installed target changes.
    ///
    /// Install implementations must verify signatures and payload policy here,
    /// even if the source paths passed inspection. Source inputs may have changed
    /// during copying. Removal has no payload to validate.
    /// # Errors
    /// Returns failed staged signature, identity or configuration validation.
    fn validate_stage(
        &mut self,
        request: &TransactionRequest,
        staged: &FilesystemStaged,
    ) -> Result<(), Self::Error>;

    /// Confirms no owned process can access the targets under retained leases.
    /// This may stop the persistent Driver; restore must recover prior registration.
    /// # Errors
    /// Returns an incomplete drain or ownership verification failure.
    fn drain(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error>;

    /// Registers the committed files, or unregisters the removed distribution.
    /// # Errors
    /// Returns a registration failure, including partially changed registration.
    fn register(&mut self, request: &TransactionRequest)
    -> Result<Self::Registration, Self::Error>;

    /// Stops and drains the current verified Driver before restoring any files.
    /// # Errors
    /// Returns incomplete drain or identity verification; cleanup is still attempted.
    fn prepare_restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error>;

    /// Restores registration after the prior files have been restored.
    /// # Errors
    /// Returns incomplete service recovery. Private staging is still cleaned up.
    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error>;
}

/// Typed failure of the composed installation boundaries.
#[derive(Debug, Error)]
pub enum CoordinatedError<E: Error + 'static> {
    /// Lifecycle acquisition or validation failed.
    #[error(transparent)]
    Lease(#[from] LeaseError),
    /// Exact-target filesystem operation failed.
    #[error(transparent)]
    Filesystem(#[from] FilesystemError),
    /// Platform trust or registration operation failed.
    #[error("platform service boundary failed: {0}")]
    Service(#[source] E),
    /// A rejected staged payload could not be cleaned up.
    #[error("staged payload validation failed: {cause}; cleanup failed: {cleanup}")]
    StageCleanup {
        /// The validation error that prevented mutation.
        #[source]
        cause: E,
        /// The cleanup error; private staging could not be removed.
        cleanup: Box<FilesystemError>,
    },
}

/// Recovery diagnostics when either files or registration could not recover.
#[derive(Debug)]
pub enum CoordinatedRecoveryDiagnostics {
    /// Files were not fully restored; registration restoration was not attempted.
    Filesystem(FilesystemRecoveryDiagnostics),
    /// Driver drain or prior service restoration failed; files may still need recovery.
    Service {
        /// The attempted transaction path; successful cleanup removes it.
        staging_root: PathBuf,
    },
}

/// One owner connecting filesystem, lifecycle guards and service operations.
///
/// This does not acquire administrator rights or supply a native service
/// implementation. Call [`crate::execute`] to preserve rollback ordering and
/// keep the guard alive through service restoration and staging cleanup.
pub struct CoordinatedBackend<S> {
    filesystem: FilesystemAdapter,
    service: S,
}

impl<S: ServiceLifecycle> CoordinatedBackend<S> {
    /// Binds a service implementation to one fixed filesystem root.
    #[must_use]
    pub fn new(layout: FilesystemLayout, service: S) -> Self {
        Self {
            filesystem: FilesystemAdapter::new(layout),
            service,
        }
    }
}

impl<S: ServiceLifecycle> PlatformBackend for CoordinatedBackend<S> {
    type LeaseGuard = LifecycleLeaseGuard;
    type Snapshot = (FilesystemSnapshot, S::Snapshot);
    type Staged = FilesystemStaged;
    type Drained = ();
    type Committed = FilesystemCommitted;
    type Registration = S::Registration;
    type RecoveryDiagnostics = CoordinatedRecoveryDiagnostics;
    type Error = CoordinatedError<S::Error>;

    fn preflight(&mut self, request: &TransactionRequest) -> PreflightResult<Self> {
        let layout = self.filesystem.layout();
        let service = self
            .service
            .inspect(request, layout)
            .map_err(|error| PreflightError::Backend(CoordinatedError::Service(error)))?;
        let directory = LifecycleDirectory::open(
            layout
                .root()
                .join(LIFECYCLE_DIRECTORY.trim_start_matches('/')),
        )
        .map_err(|error| PreflightError::Backend(CoordinatedError::Lease(error)))?;
        let guard = directory.acquire().map_err(|error| match error {
            LeaseError::Busy { kind } => PreflightError::LeaseBusy { kind },
            error => PreflightError::Backend(CoordinatedError::Lease(error)),
        })?;
        // Capture service state again under installer serialization: the first
        // inspection authorizes opening lease files but is not the rollback snapshot.
        drop(service);
        let service = self
            .service
            .inspect(request, layout)
            .map_err(|error| PreflightError::Backend(CoordinatedError::Service(error)))?;
        // A previous transaction may have restored every original file but
        // failed its service recovery check. Discard that abandoned transaction
        // only after trust inspection and while all lifecycle leases are held.
        self.filesystem
            .cleanup_stale_staging()
            .map_err(|error| PreflightError::Backend(CoordinatedError::Filesystem(error)))?;
        let files = self
            .filesystem
            .preflight(request)
            .map_err(|error| PreflightError::Backend(CoordinatedError::Filesystem(error)))?;
        Ok(Preflight::new(guard, (files, service)))
    }

    fn stage(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
    ) -> Result<Self::Staged, Self::Error> {
        let staged = self.filesystem.stage(request, &preflight.snapshot().0)?;
        if let Err(cause) = self.service.validate_stage(request, &staged) {
            return match self.filesystem.cleanup(&staged) {
                Ok(()) => Err(CoordinatedError::Service(cause)),
                Err(cleanup) => Err(CoordinatedError::StageCleanup {
                    cause,
                    cleanup: Box::new(cleanup),
                }),
            };
        }
        Ok(staged)
    }

    fn drain(
        &mut self,
        _request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
    ) -> Result<(), Self::Error> {
        self.service
            .drain(&preflight.snapshot().1)
            .map_err(CoordinatedError::Service)
    }

    fn commit(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        staged: &Self::Staged,
        _drained: &(),
    ) -> Result<Self::Committed, Self::Error> {
        Ok(self
            .filesystem
            .commit(request, &preflight.snapshot().0, staged)?)
    }

    fn register(
        &mut self,
        request: &TransactionRequest,
        _preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        _committed: &Self::Committed,
    ) -> Result<Self::Registration, Self::Error> {
        self.service
            .register(request)
            .map_err(CoordinatedError::Service)
    }

    fn rollback(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        staged: &Self::Staged,
        committed: Option<&Self::Committed>,
    ) -> Result<(), RollbackFailure<Self::Error, Self::RecoveryDiagnostics>> {
        self.service
            .prepare_restore(&preflight.snapshot().1)
            .map_err(|error| {
                RollbackFailure::new(
                    CoordinatedError::Service(error),
                    CoordinatedRecoveryDiagnostics::Service {
                        staging_root: staged.staging_root().to_path_buf(),
                    },
                )
            })?;
        self.filesystem
            .rollback(request, &preflight.snapshot().0, staged, committed)
            .map_err(|failure| {
                let (error, diagnostics) = failure.into_parts();
                RollbackFailure::new(
                    CoordinatedError::Filesystem(error),
                    CoordinatedRecoveryDiagnostics::Filesystem(diagnostics),
                )
            })?;
        // A registration attempt may have started the new Driver. It must be
        // drained before file recovery; only then can the old runtime be restored.
        self.service
            .restore(&preflight.snapshot().1)
            .map_err(|error| {
                RollbackFailure::new(
                    CoordinatedError::Service(error),
                    CoordinatedRecoveryDiagnostics::Service {
                        staging_root: staged.staging_root().to_path_buf(),
                    },
                )
            })
    }

    fn cleanup(
        &mut self,
        _request: &TransactionRequest,
        _preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        staged: Self::Staged,
    ) -> Result<(), Self::Error> {
        Ok(self.filesystem.cleanup(&staged)?)
    }
}

#[cfg(test)]
mod tests;
