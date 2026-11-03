//! Single-operation transaction state machine and recovery reporting.

use std::fmt::{self, Display, Formatter};

use thiserror::Error;

use crate::{
    backend::{LeaseKind, PlatformBackend, Preflight, PreflightError},
    request::{OperationKind, TransactionRequest},
};

/// The last completed boundary of a transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionState {
    /// No backend operation has started.
    Created,
    /// Read-only validation and shared lease acquisition completed.
    Preflighted,
    /// Private staging completed.
    Staged,
    /// App and engine drain completed.
    Drained,
    /// Exact owned targets were replaced or removed.
    Committed,
    /// Exact launchd registration or removal completed.
    Registered,
    /// Private staging was cleaned after registration.
    Completed,
}

/// A drain, commit or registration boundary that triggered recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryPhase {
    /// Stopping the persistent Driver failed after a possible registration change.
    Drain,
    /// Commit returned an error after a possible partial replacement.
    Commit,
    /// Registration returned an error after owned targets were committed.
    Registration,
}

/// Result of private staging cleanup.
#[derive(Debug)]
pub enum CleanupStatus<E> {
    /// Private staging was removed successfully.
    Succeeded,
    /// Private staging removal failed and must be reported.
    Failed(E),
}

impl<E: Display> Display for CleanupStatus<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded => formatter.write_str("staging cleanup succeeded"),
            Self::Failed(error) => write!(formatter, "staging cleanup failed: {error}"),
        }
    }
}

/// Outcome of rollback and private staging cleanup after a mutation failure.
#[derive(Debug)]
pub enum RecoveryStatus<E, R> {
    /// The preflight snapshot was restored; cleanup may still have failed.
    Restored { cleanup: CleanupStatus<E> },
    /// Rollback failed; diagnostics and mandatory staging cleanup are reported.
    Failed {
        /// The rollback failure that prevents a success claim.
        rollback: E,
        /// Rollback diagnostics; referenced staging paths may already be removed.
        diagnostics: R,
        /// Cleanup is attempted even when rollback cannot restore the snapshot.
        cleanup: CleanupStatus<E>,
    },
}

impl<E: Display, R> Display for RecoveryStatus<E, R> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restored { cleanup } => write!(formatter, "rollback restored targets; {cleanup}"),
            Self::Failed {
                rollback, cleanup, ..
            } => write!(formatter, "rollback failed: {rollback}; {cleanup}"),
        }
    }
}

/// A transaction failure that never claims successful installation or removal.
#[derive(Debug, Error)]
pub enum TransactionError<E, R>
where
    E: std::error::Error + 'static,
{
    /// Read-only platform validation failed before staging.
    #[error("preflight failed: {0}")]
    Preflight(#[source] E),
    /// An exclusive app, engine, or installer owner was observed before staging.
    #[error("lifecycle lease is busy: {kind:?}")]
    LeaseBusy {
        /// The lease that blocked preflight.
        kind: LeaseKind,
    },
    /// Private staging failed before the drain boundary.
    #[error("staging failed: {0}")]
    Stage(#[source] E),
    /// Drain, commit or registration failed and recovery was attempted.
    #[error("{phase:?} failed: {cause}; {recovery}")]
    MutationRecovery {
        /// The mutation boundary that failed.
        phase: RecoveryPhase,
        /// The original mutation failure.
        #[source]
        cause: E,
        /// Rollback diagnostics and cleanup results.
        recovery: RecoveryStatus<E, R>,
    },
    /// Registration succeeded but private staging cleanup failed.
    #[error("transaction completed its owned mutation but cleanup failed: {0}")]
    Cleanup(#[source] E),
}

impl<E, R> TransactionError<E, R>
where
    E: std::error::Error + 'static,
{
    /// Returns the last completed state before this error was returned.
    #[must_use]
    pub const fn state(&self) -> TransactionState {
        match self {
            Self::Preflight(_) | Self::LeaseBusy { .. } => TransactionState::Created,
            Self::Stage(_) => TransactionState::Preflighted,
            Self::MutationRecovery { phase, .. } => match phase {
                RecoveryPhase::Drain => TransactionState::Staged,
                RecoveryPhase::Commit => TransactionState::Drained,
                RecoveryPhase::Registration => TransactionState::Committed,
            },
            Self::Cleanup(_) => TransactionState::Registered,
        }
    }
}

/// Successful result returned only after registration and private cleanup.
#[derive(Debug)]
pub struct TransactionOutcome<R> {
    operation: OperationKind,
    registration: R,
    state: TransactionState,
}

/// Result type returned by [`execute`] for a concrete backend.
pub type ExecutionResult<B> = Result<
    TransactionOutcome<<B as PlatformBackend>::Registration>,
    TransactionError<<B as PlatformBackend>::Error, <B as PlatformBackend>::RecoveryDiagnostics>,
>;

impl<R> TransactionOutcome<R> {
    /// Returns the operation that completed.
    #[must_use]
    pub const fn operation(&self) -> OperationKind {
        self.operation
    }

    /// Returns the backend registration receipt.
    #[must_use]
    pub fn registration(&self) -> &R {
        &self.registration
    }

    /// Returns the final completed state.
    #[must_use]
    pub const fn state(&self) -> TransactionState {
        self.state
    }
}

/// Runs exactly one coordinated install or remove operation.
///
/// The backend's shared lease guard remains held from successful preflight
/// through cleanup or rollback. Every backend boundary is called at most once;
/// retry policy belongs outside this core and is intentionally absent here.
///
/// # Errors
/// Returns a typed boundary failure. Commit and registration failures always
/// include rollback and private-staging cleanup results. Cleanup is attempted
/// under retained leases after every rollback result, including failure.
pub fn execute<B: PlatformBackend>(
    backend: &mut B,
    request: &TransactionRequest,
) -> ExecutionResult<B> {
    let preflight = match backend.preflight(request) {
        Ok(preflight) => preflight,
        Err(PreflightError::LeaseBusy { kind }) => {
            return Err(TransactionError::LeaseBusy { kind });
        }
        Err(PreflightError::Backend(error)) => return Err(TransactionError::Preflight(error)),
    };
    let staged = backend
        .stage(request, &preflight)
        .map_err(TransactionError::Stage)?;
    let drained = match backend.drain(request, &preflight) {
        Ok(drained) => drained,
        Err(cause) => {
            let recovery = recover(backend, request, &preflight, staged, None);
            return Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Drain,
                cause,
                recovery,
            });
        }
    };
    let committed = match backend.commit(request, &preflight, &staged, &drained) {
        Ok(committed) => committed,
        Err(cause) => {
            let recovery = recover(backend, request, &preflight, staged, None);
            return Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Commit,
                cause,
                recovery,
            });
        }
    };
    let registration = match backend.register(request, &preflight, &committed) {
        Ok(registration) => registration,
        Err(cause) => {
            let recovery = recover(backend, request, &preflight, staged, Some(&committed));
            return Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Registration,
                cause,
                recovery,
            });
        }
    };
    backend
        .cleanup(request, &preflight, staged)
        .map_err(TransactionError::Cleanup)?;
    Ok(TransactionOutcome {
        operation: request.kind(),
        registration,
        state: TransactionState::Completed,
    })
}

fn cleanup_status<B: PlatformBackend>(
    backend: &mut B,
    request: &TransactionRequest,
    preflight: &Preflight<B::LeaseGuard, B::Snapshot>,
    staged: B::Staged,
) -> CleanupStatus<B::Error> {
    match backend.cleanup(request, preflight, staged) {
        Ok(()) => CleanupStatus::Succeeded,
        Err(error) => CleanupStatus::Failed(error),
    }
}

fn recover<B: PlatformBackend>(
    backend: &mut B,
    request: &TransactionRequest,
    preflight: &Preflight<B::LeaseGuard, B::Snapshot>,
    staged: B::Staged,
    committed: Option<&B::Committed>,
) -> RecoveryStatus<B::Error, B::RecoveryDiagnostics> {
    let rollback_result = backend.rollback(request, preflight, &staged, committed);
    let cleanup = cleanup_status(backend, request, preflight, staged);
    match rollback_result {
        Ok(()) => RecoveryStatus::Restored { cleanup },
        Err(failure) => {
            let (rollback, diagnostics) = failure.into_parts();
            RecoveryStatus::Failed {
                rollback,
                diagnostics,
                cleanup,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Failure {
        Preflight,
        Stage,
        Drain,
        Commit,
        Register,
        Rollback,
        Cleanup,
    }

    #[derive(Debug, Error)]
    #[error("fake backend failure at {0:?}")]
    struct FakeError(Failure);

    #[derive(Clone, Debug)]
    struct FakeGuard {
        events: Rc<RefCell<Vec<&'static str>>>,
        live: Rc<Cell<bool>>,
    }

    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.live.set(false);
            self.events.borrow_mut().push("guard-drop");
        }
    }

    #[derive(Debug)]
    struct FakeBackend {
        events: Rc<RefCell<Vec<&'static str>>>,
        failures: Vec<Failure>,
        busy: Option<LeaseKind>,
        last_kind: Option<OperationKind>,
        guard_live: Rc<Cell<bool>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                events: Rc::new(RefCell::new(Vec::new())),
                failures: Vec::new(),
                busy: None,
                last_kind: None,
                guard_live: Rc::new(Cell::new(false)),
            }
        }

        fn fails_at(mut self, failure: Failure) -> Self {
            self.failures.push(failure);
            self
        }

        fn busy(mut self, busy: LeaseKind) -> Self {
            self.busy = Some(busy);
            self
        }

        fn event(&self, event: &'static str) {
            self.events.borrow_mut().push(event);
        }

        fn fail_if_configured(&self, failure: Failure) -> Result<(), FakeError> {
            if self.failures.contains(&failure) {
                Err(FakeError(failure))
            } else {
                Ok(())
            }
        }

        fn events(&self) -> Vec<&'static str> {
            self.events.borrow().clone()
        }

        fn assert_guard_live(preflight: &Preflight<FakeGuard, &'static str>) {
            assert!(preflight.lease_guard().live.get());
        }
    }

    impl PlatformBackend for FakeBackend {
        type LeaseGuard = FakeGuard;
        type Snapshot = &'static str;
        type Staged = &'static str;
        type Drained = &'static str;
        type Committed = &'static str;
        type Registration = &'static str;
        type RecoveryDiagnostics = &'static str;
        type Error = FakeError;

        fn preflight(
            &mut self,
            request: &TransactionRequest,
        ) -> Result<Preflight<Self::LeaseGuard, Self::Snapshot>, PreflightError<Self::Error>>
        {
            self.event("preflight");
            self.last_kind = Some(request.kind());
            if let Some(kind) = self.busy {
                return Err(PreflightError::LeaseBusy { kind });
            }
            self.fail_if_configured(Failure::Preflight)
                .map_err(PreflightError::Backend)?;
            self.guard_live.set(true);
            Ok(Preflight::new(
                FakeGuard {
                    events: Rc::clone(&self.events),
                    live: Rc::clone(&self.guard_live),
                },
                "snapshot",
            ))
        }

        fn stage(
            &mut self,
            _request: &TransactionRequest,
            preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        ) -> Result<Self::Staged, Self::Error> {
            Self::assert_guard_live(preflight);
            self.event("stage");
            self.fail_if_configured(Failure::Stage)?;
            Ok("staged")
        }

        fn drain(
            &mut self,
            _request: &TransactionRequest,
            preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
        ) -> Result<Self::Drained, Self::Error> {
            Self::assert_guard_live(preflight);
            self.event("drain");
            self.fail_if_configured(Failure::Drain)?;
            Ok("drained")
        }

        fn commit(
            &mut self,
            _request: &TransactionRequest,
            preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
            _staged: &Self::Staged,
            _drained: &Self::Drained,
        ) -> Result<Self::Committed, Self::Error> {
            Self::assert_guard_live(preflight);
            self.event("commit");
            self.fail_if_configured(Failure::Commit)?;
            Ok("committed")
        }

        fn register(
            &mut self,
            _request: &TransactionRequest,
            preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
            _committed: &Self::Committed,
        ) -> Result<Self::Registration, Self::Error> {
            Self::assert_guard_live(preflight);
            self.event("register");
            self.fail_if_configured(Failure::Register)?;
            Ok("registered")
        }

        fn rollback(
            &mut self,
            _request: &TransactionRequest,
            preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
            _staged: &Self::Staged,
            _committed: Option<&Self::Committed>,
        ) -> Result<(), crate::RollbackFailure<Self::Error, Self::RecoveryDiagnostics>> {
            Self::assert_guard_live(preflight);
            self.event("rollback");
            self.fail_if_configured(Failure::Rollback)
                .map_err(|error| crate::RollbackFailure::new(error, "rollback-diagnostics"))
        }

        fn cleanup(
            &mut self,
            _request: &TransactionRequest,
            preflight: &Preflight<Self::LeaseGuard, Self::Snapshot>,
            _staged: Self::Staged,
        ) -> Result<(), Self::Error> {
            Self::assert_guard_live(preflight);
            self.event("cleanup");
            self.fail_if_configured(Failure::Cleanup)
        }
    }

    fn install_request() -> TransactionRequest {
        TransactionRequest::install(crate::InstallSources::new(
            "app",
            "hal",
            ("driver", "launchd"),
            ("remover", "removal-launchd"),
            ("engine", "engine-launchd"),
        ))
    }

    #[test]
    fn success_orders_boundaries_and_releases_shared_guard_last() {
        let mut backend = FakeBackend::new();
        let outcome = execute(&mut backend, &install_request()).expect("transaction succeeds");

        assert_eq!(outcome.operation(), OperationKind::Install);
        assert_eq!(outcome.state(), TransactionState::Completed);
        assert_eq!(outcome.registration(), &"registered");
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "commit",
                "register",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn busy_lease_stops_before_staging_or_mutation() {
        let mut backend = FakeBackend::new().busy(LeaseKind::App);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(
            result,
            Err(TransactionError::LeaseBusy {
                kind: LeaseKind::App
            })
        ));
        assert_eq!(backend.events(), vec!["preflight"]);
    }

    #[test]
    fn installer_lease_contention_stops_before_staging() {
        let mut backend = FakeBackend::new().busy(LeaseKind::Installer);
        let error = execute(&mut backend, &install_request()).expect_err("busy installer lease");

        assert_eq!(error.state(), TransactionState::Created);
        assert!(matches!(
            error,
            TransactionError::LeaseBusy {
                kind: LeaseKind::Installer
            }
        ));
        assert_eq!(backend.events(), vec!["preflight"]);
    }

    #[test]
    fn preflight_failure_has_no_guard_or_mutation() {
        let mut backend = FakeBackend::new().fails_at(Failure::Preflight);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(result, Err(TransactionError::Preflight(_))));
        assert_eq!(backend.events(), vec!["preflight"]);
    }

    #[test]
    fn stage_failure_stops_before_drain() {
        let mut backend = FakeBackend::new().fails_at(Failure::Stage);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(result, Err(TransactionError::Stage(_))));
        assert_eq!(backend.events(), vec!["preflight", "stage", "guard-drop"]);
    }

    #[test]
    fn drain_failure_restores_registration_and_cleans_stage_without_commit() {
        let mut backend = FakeBackend::new().fails_at(Failure::Drain);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(
            result,
            Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Drain,
                ..
            })
        ));
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "rollback",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn commit_failure_reports_successful_rollback() {
        let mut backend = FakeBackend::new().fails_at(Failure::Commit);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(
            result,
            Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Commit,
                recovery: RecoveryStatus::Restored {
                    cleanup: CleanupStatus::Succeeded
                },
                ..
            })
        ));
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "commit",
                "rollback",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn rollback_failure_still_cleans_staging_before_releasing_leases() {
        let mut backend = FakeBackend::new()
            .fails_at(Failure::Commit)
            .fails_at(Failure::Rollback);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(
            result,
            Err(TransactionError::MutationRecovery {
                recovery: RecoveryStatus::Failed {
                    rollback: FakeError(Failure::Rollback),
                    diagnostics: "rollback-diagnostics",
                    cleanup: CleanupStatus::Succeeded,
                },
                ..
            })
        ));
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "commit",
                "rollback",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn registration_rollback_failure_still_cleans_staging() {
        let mut backend = FakeBackend::new()
            .fails_at(Failure::Register)
            .fails_at(Failure::Rollback);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(
            result,
            Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Registration,
                recovery: RecoveryStatus::Failed {
                    rollback: FakeError(Failure::Rollback),
                    diagnostics: "rollback-diagnostics",
                    cleanup: CleanupStatus::Succeeded,
                },
                ..
            })
        ));
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "commit",
                "register",
                "rollback",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn registration_failure_rolls_back_the_committed_set() {
        let mut backend = FakeBackend::new().fails_at(Failure::Register);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(
            result,
            Err(TransactionError::MutationRecovery {
                phase: RecoveryPhase::Registration,
                recovery: RecoveryStatus::Restored {
                    cleanup: CleanupStatus::Succeeded
                },
                ..
            })
        ));
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "commit",
                "register",
                "rollback",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn cleanup_failure_is_not_reported_as_success() {
        let mut backend = FakeBackend::new().fails_at(Failure::Cleanup);
        let result = execute(&mut backend, &install_request());

        assert!(matches!(result, Err(TransactionError::Cleanup(_))));
        assert_eq!(
            backend.events(),
            vec![
                "preflight",
                "stage",
                "drain",
                "commit",
                "register",
                "cleanup",
                "guard-drop"
            ]
        );
    }

    #[test]
    fn rollback_and_cleanup_failures_are_both_reported_under_leases() {
        let mut backend = FakeBackend::new()
            .fails_at(Failure::Register)
            .fails_at(Failure::Rollback)
            .fails_at(Failure::Cleanup);
        let error = execute(&mut backend, &install_request()).unwrap_err();
        assert!(matches!(
            error,
            TransactionError::MutationRecovery {
                recovery: RecoveryStatus::Failed {
                    rollback: FakeError(Failure::Rollback),
                    cleanup: CleanupStatus::Failed(FakeError(Failure::Cleanup)),
                    ..
                },
                ..
            }
        ));
        let description = error.to_string();
        assert!(description.contains("rollback failed"));
        assert!(description.contains("staging cleanup failed"));
        assert_eq!(
            &backend.events()[5..],
            &["rollback", "cleanup", "guard-drop"]
        );
    }

    #[test]
    fn remove_request_has_no_source_paths_or_unrelated_target_operation() {
        let mut backend = FakeBackend::new();
        let result = execute(&mut backend, &TransactionRequest::remove()).expect("remove succeeds");

        assert_eq!(result.operation(), OperationKind::Remove);
        assert_eq!(backend.last_kind, Some(OperationKind::Remove));
        assert_eq!(
            crate::OwnedTarget::all().map(crate::OwnedTarget::path),
            [
                crate::APP_PATH,
                crate::DRIVER_PATH,
                crate::HAL_PATH,
                crate::LAUNCHD_PATH,
                crate::UNINSTALLER_PATH,
                crate::REMOVAL_LAUNCHD_PATH,
                crate::RUNTIME_ENGINE_PATH,
                crate::RUNTIME_LAUNCHD_PATH,
            ]
        );
    }
}
