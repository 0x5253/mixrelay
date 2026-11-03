//! Platform boundary for the pure transaction orchestrator.

use std::error::Error;

use thiserror::Error;

use crate::request::TransactionRequest;

/// A fixed lifecycle lock that can block a transaction preflight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseKind {
    /// The desktop's exclusive process owner is active.
    App,
    /// The engine's exclusive process owner is active.
    Engine,
    /// Another installer transaction owns the administrator serialization lock.
    Installer,
}

/// A completed read-only preflight and its retained shared lease guard.
///
/// The guard must hold locks on all fixed lease inodes until the transaction
/// has either completed cleanup or finished rollback. The guard holds shared
/// locks on the app and engine lease inodes plus an exclusive lock on the
/// installer lease inode. The backend may create a missing
/// initial-install inode before acquiring that guard, but it must never replace
/// an existing inode.
#[derive(Debug)]
pub struct Preflight<G, S> {
    lease_guard: G,
    snapshot: S,
}

impl<G, S> Preflight<G, S> {
    /// Creates a preflight result owned by the transaction until completion.
    #[must_use]
    pub fn new(lease_guard: G, snapshot: S) -> Self {
        Self {
            lease_guard,
            snapshot,
        }
    }

    /// Returns the backend-owned shared lease guard.
    #[must_use]
    pub fn lease_guard(&self) -> &G {
        &self.lease_guard
    }

    /// Returns the immutable snapshot of exact owned targets.
    #[must_use]
    pub fn snapshot(&self) -> &S {
        &self.snapshot
    }
}

/// Distinguishes a lease rejection from another read-only preflight failure.
#[derive(Debug, Error)]
pub enum PreflightError<E>
where
    E: Error + 'static,
{
    /// A fixed lifecycle lease was busy; no staging occurred.
    #[error("lifecycle lease is busy: {kind:?}")]
    LeaseBusy {
        /// The lease that blocked preflight.
        kind: LeaseKind,
    },
    /// The backend could not complete its read-only artifact and ownership checks.
    #[error("platform preflight failed: {0}")]
    Backend(#[source] E),
}

/// A failed rollback together with diagnostics captured before staging cleanup.
#[derive(Debug)]
pub struct RollbackFailure<E, R> {
    error: E,
    diagnostics: R,
}

impl<E, R> RollbackFailure<E, R> {
    /// Creates a rollback failure with diagnostics captured before cleanup.
    #[must_use]
    pub fn new(error: E, diagnostics: R) -> Self {
        Self { error, diagnostics }
    }

    /// Returns the rollback failure.
    #[must_use]
    pub fn error(&self) -> &E {
        &self.error
    }

    /// Returns rollback diagnostics, whose staging paths may already be removed.
    #[must_use]
    pub fn diagnostics(&self) -> &R {
        &self.diagnostics
    }

    /// Consumes the failure and returns both its error and recovery diagnostics.
    #[must_use]
    pub fn into_parts(self) -> (E, R) {
        (self.error, self.diagnostics)
    }
}

/// Injected platform operations used by the transaction state machine.
///
/// Implementations own all filesystem, launchd, `CoreAudio`, and signature
/// details. The orchestrator invokes each operation at most once and never
/// supplies arbitrary commands from an unsigned payload.
pub trait PlatformBackend {
    /// Guard retaining shared app/engine locks and an exclusive installer lock.
    type LeaseGuard;
    /// Backend-specific exact-target snapshot captured during preflight.
    type Snapshot;
    /// Private staging handle; cleanup consumes it.
    type Staged;
    /// Receipt proving the platform drain boundary completed.
    type Drained;
    /// Receipt describing the exact owned replacement/removal commit.
    type Committed;
    /// Receipt describing launchd registration or removal.
    type Registration;
    /// Backend diagnostics when rollback cannot restore targets.
    type RecoveryDiagnostics;
    /// Backend failure type, retaining platform error context.
    type Error: Error + Send + Sync + 'static;

    /// Validates sources, exact owned targets, user data, and lease state.
    ///
    /// This method must not mutate owned artifacts or user data. It may create
    /// missing initial-install lease inodes before taking the guard, provided
    /// the containing root-owned directory and inode types are validated first.
    /// A successful result must retain shared app/engine locks and an exclusive
    /// installer lock in the returned [`Preflight`].
    ///
    /// # Errors
    /// Returns a lease-busy rejection or a backend validation failure.
    fn preflight(&mut self, request: &TransactionRequest) -> PreflightResult<Self>;

    /// Creates a private replacement/removal staging handle.
    ///
    /// A failed implementation must clean any partial private staging itself;
    /// no owned target may be changed before `commit`.
    ///
    /// # Errors
    /// Returns the backend staging failure.
    fn stage(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
    ) -> Result<Self::Staged, Self::Error>;

    /// Stops and drains the exact verified Driver under retained app/engine locks,
    /// without killing unrelated processes or restarting `CoreAudio`.
    ///
    /// A failure may have stopped registration; rollback restores the snapshot.
    ///
    /// # Errors
    /// Returns the backend drain failure.
    fn drain(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
    ) -> Result<Self::Drained, Self::Error>;

    /// Performs the coordinated, recoverable replacement or removal of only the
    /// eight exact owned targets. The eight paths are not one filesystem atomic
    /// operation; a later failure is handled through `rollback`.
    ///
    /// # Errors
    /// Returns the backend commit failure; the orchestrator then attempts rollback.
    fn commit(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        staged: &Self::Staged,
        drained: &Self::Drained,
    ) -> Result<Self::Committed, Self::Error>;

    /// Registers or unregisters only the exact `MixRelay` launchd service.
    ///
    /// # Errors
    /// Returns the backend registration failure; the orchestrator then attempts rollback.
    fn register(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        committed: &Self::Committed,
    ) -> Result<Self::Registration, Self::Error>;

    /// Restores the preflight snapshot after a commit or registration failure.
    ///
    /// `committed` is absent when commit returned an error after a partial
    /// mutation and present when registration returned an error. On failure,
    /// the returned [`RollbackFailure`] reports recovery diagnostics. The
    /// orchestrator still calls `cleanup` before releasing lifecycle leases;
    /// rollback failure never exempts temporary backups from deletion.
    /// Any newly started owned process must be drained before restoring files;
    /// prior native runtime and registration are restored only afterward.
    ///
    /// # Errors
    /// Returns a rollback failure with recovery diagnostics.
    fn rollback(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        staged: &Self::Staged,
        committed: Option<&Self::Committed>,
    ) -> Result<(), RollbackFailure<Self::Error, Self::RecoveryDiagnostics>>;

    /// Removes private staging after success or any recovery attempt, even failure.
    ///
    /// # Errors
    /// Returns the backend cleanup failure.
    fn cleanup(
        &mut self,
        request: &TransactionRequest,
        preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        staged: Self::Staged,
    ) -> Result<(), Self::Error>;
}

/// Result type returned by a backend's read-only preflight.
pub type PreflightResult<B> = Result<
    Preflight<<B as PlatformBackend>::LeaseGuard, <B as PlatformBackend>::Snapshot>,
    PreflightError<<B as PlatformBackend>::Error>,
>;
