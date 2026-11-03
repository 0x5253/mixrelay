use super::*;
use crate::{
    APP_PATH, DRIVER_PATH, HAL_PATH, LAUNCHD_PATH, OwnedTarget, RecoveryStatus, TransactionError,
    execute,
};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    _root: mixrelay_test_support::FixtureDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root_owner = mixrelay_test_support::FixtureDir::new("macos-coordinated").unwrap();
        let root = root_owner.path().to_path_buf();
        let lifecycle = root.join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
        fs::create_dir_all(&lifecycle).unwrap();
        fs::set_permissions(&lifecycle, fs::Permissions::from_mode(0o700)).unwrap();
        for target in OwnedTarget::all() {
            let path = root.join(target.path().trim_start_matches('/'));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if matches!(
                target,
                OwnedTarget::App | OwnedTarget::Hal | OwnedTarget::Uninstaller
            ) {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("owned"), b"original").unwrap();
            } else {
                fs::write(path, b"original").unwrap();
            }
        }
        fs::write(root.join("user-graph.json"), b"keep").unwrap();
        Self {
            _root: root_owner,
            path: root,
        }
    }

    fn backend(&self, fail_register: bool, fail_restore: bool) -> CoordinatedBackend<Service> {
        CoordinatedBackend::new(
            FilesystemLayout::new(&self.path).unwrap(),
            Service {
                root: self.path.clone(),
                fail_register,
                fail_restore,
                stage_failure: None,
                approved: true,
                events: Vec::new(),
            },
        )
    }
}

#[derive(Debug, Error)]
#[error("injected service failure")]
struct ServiceError;

struct Service {
    root: PathBuf,
    fail_register: bool,
    fail_restore: bool,
    stage_failure: Option<StageFailure>,
    approved: bool,
    events: Vec<&'static str>,
}

#[derive(Clone, Copy)]
enum StageFailure {
    Reject,
    RejectWithCleanupFailure,
    PrepareRestore,
}

impl Service {
    fn assert_locked(&self) {
        let directory = self.root.join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
        for name in ["app.owner", "engine.owner", "installer.owner"] {
            let file = fs::File::open(directory.join(name)).unwrap();
            assert!(matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)));
        }
    }
}

impl ServiceLifecycle for Service {
    type Snapshot = ();
    type Registration = ();
    type Error = ServiceError;
    fn inspect(
        &mut self,
        _request: &TransactionRequest,
        _layout: &FilesystemLayout,
    ) -> Result<(), ServiceError> {
        self.events.push("inspect");
        if !self.approved {
            return Err(ServiceError);
        }
        Ok(())
    }
    fn drain(&mut self, _snapshot: &()) -> Result<(), ServiceError> {
        self.assert_locked();
        self.events.push("drain");
        assert!(self.root.join(APP_PATH.trim_start_matches('/')).exists());
        Ok(())
    }
    fn validate_stage(
        &mut self,
        _request: &TransactionRequest,
        staged: &FilesystemStaged,
    ) -> Result<(), ServiceError> {
        self.assert_locked();
        self.events.push("validate_stage");
        if matches!(
            self.stage_failure,
            Some(StageFailure::RejectWithCleanupFailure)
        ) {
            fs::rename(
                staged.staging_root(),
                staged.staging_root().with_extension("displaced"),
            )
            .unwrap();
            fs::create_dir(staged.staging_root()).unwrap();
        }
        if matches!(
            self.stage_failure,
            Some(StageFailure::Reject | StageFailure::RejectWithCleanupFailure)
        ) {
            Err(ServiceError)
        } else {
            Ok(())
        }
    }
    fn register(&mut self, _request: &TransactionRequest) -> Result<(), ServiceError> {
        self.assert_locked();
        self.events.push("register");
        for path in [APP_PATH, DRIVER_PATH, HAL_PATH, LAUNCHD_PATH] {
            assert!(!self.root.join(path.trim_start_matches('/')).exists());
        }
        if self.fail_register {
            Err(ServiceError)
        } else {
            Ok(())
        }
    }
    fn prepare_restore(&mut self, _snapshot: &()) -> Result<(), ServiceError> {
        self.assert_locked();
        self.events.push("prepare_restore");
        // A registration failure happens after commit. No file restoration may
        // precede the callback that drains any newly started Driver.
        if self.events.contains(&"register") {
            for target in OwnedTarget::all() {
                assert!(
                    !self
                        .root
                        .join(target.path().trim_start_matches('/'))
                        .exists()
                );
            }
        }
        if matches!(self.stage_failure, Some(StageFailure::PrepareRestore)) {
            return Err(ServiceError);
        }
        Ok(())
    }
    fn restore(&mut self, _snapshot: &()) -> Result<(), ServiceError> {
        self.assert_locked();
        self.events.push("restore");
        for path in [APP_PATH, DRIVER_PATH, HAL_PATH, LAUNCHD_PATH] {
            assert!(self.root.join(path.trim_start_matches('/')).exists());
        }
        if self.fail_restore {
            Err(ServiceError)
        } else {
            Ok(())
        }
    }
}

#[test]
fn removal_holds_leases_through_service_registration_and_preserves_user_data() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(false, false);
    execute(&mut backend, &TransactionRequest::remove()).unwrap();
    assert!(!backend.filesystem.layout().staging_root().exists());
    assert_eq!(
        fs::read(fixture.path.join("user-graph.json")).unwrap(),
        b"keep"
    );
    drop(
        LifecycleDirectory::open(
            fixture
                .path
                .join(LIFECYCLE_DIRECTORY.trim_start_matches('/')),
        )
        .unwrap()
        .acquire()
        .unwrap(),
    );
}

#[test]
fn registration_failure_restores_files_before_service_and_removes_private_staging() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(true, false);
    assert!(matches!(
        execute(&mut backend, &TransactionRequest::remove()),
        Err(TransactionError::MutationRecovery {
            recovery: RecoveryStatus::Restored { .. },
            ..
        })
    ));
    assert!(!backend.filesystem.layout().staging_root().exists());
    assert_eq!(
        backend.service.events,
        [
            "inspect",
            "inspect",
            "validate_stage",
            "drain",
            "register",
            "prepare_restore",
            "restore",
        ]
    );
    assert_eq!(
        fs::read(fixture.path.join(DRIVER_PATH.trim_start_matches('/'))).unwrap(),
        b"original"
    );
}

#[test]
fn missing_approval_prevents_leases_staging_and_target_mutations() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(false, false);
    backend.service.approved = false;
    assert!(execute(&mut backend, &TransactionRequest::remove()).is_err());
    assert_eq!(backend.service.events, ["inspect"]);
    assert!(!backend.filesystem.layout().staging_root().exists());
    let lifecycle = fixture
        .path
        .join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
    assert_eq!(fs::read_dir(lifecycle).unwrap().count(), 0);
    for target in OwnedTarget::all() {
        assert!(
            fixture
                .path
                .join(target.path().trim_start_matches('/'))
                .exists()
        );
    }
}

#[test]
fn rollback_drain_failure_cleans_backups_without_restoring_live_files() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(true, false);
    backend.service.stage_failure = Some(StageFailure::PrepareRestore);
    assert!(matches!(
        execute(&mut backend, &TransactionRequest::remove()),
        Err(TransactionError::MutationRecovery {
            recovery: RecoveryStatus::Failed {
                diagnostics: CoordinatedRecoveryDiagnostics::Service { .. },
                ..
            },
            ..
        })
    ));
    assert!(!backend.filesystem.layout().staging_root().exists());
    assert_eq!(backend.service.events.last(), Some(&"prepare_restore"));
    for target in OwnedTarget::all() {
        assert!(
            !fixture
                .path
                .join(target.path().trim_start_matches('/'))
                .exists()
        );
    }
}

#[test]
fn service_restore_failure_cleans_staging_without_claiming_recovery() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(true, true);
    let result = execute(&mut backend, &TransactionRequest::remove());
    match result {
        Err(TransactionError::MutationRecovery {
            recovery:
                RecoveryStatus::Failed {
                    diagnostics: CoordinatedRecoveryDiagnostics::Service { staging_root },
                    ..
                },
            ..
        }) => {
            assert!(!staging_root.exists());
            assert_eq!(staging_root, backend.filesystem.layout().staging_root());
        }
        result => panic!("unexpected recovery result: {result:?}"),
    }
}

#[test]
fn busy_installer_prevents_staging_and_target_changes() {
    let fixture = Fixture::new();
    let directory = LifecycleDirectory::open(
        fixture
            .path
            .join(LIFECYCLE_DIRECTORY.trim_start_matches('/')),
    )
    .unwrap();
    let _guard = directory.acquire().unwrap();
    let mut backend = fixture.backend(false, false);
    assert!(matches!(
        execute(&mut backend, &TransactionRequest::remove()),
        Err(TransactionError::LeaseBusy {
            kind: crate::LeaseKind::Installer
        })
    ));
    assert!(!backend.filesystem.layout().staging_root().exists());
    assert!(fixture.path.join(APP_PATH.trim_start_matches('/')).is_dir());
}

#[test]
fn abandoned_staging_cleanup_waits_for_installer_serialization() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(false, false);
    let staging = backend.filesystem.layout().staging_root().to_path_buf();
    fs::create_dir(&staging).unwrap();
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(staging.join("backups")).unwrap();
    fs::create_dir(staging.join("payload")).unwrap();
    fs::write(staging.join("backups/driver"), b"old transaction").unwrap();
    let directory = LifecycleDirectory::open(
        fixture
            .path
            .join(LIFECYCLE_DIRECTORY.trim_start_matches('/')),
    )
    .unwrap();
    let guard = directory.acquire().unwrap();
    assert!(matches!(
        execute(&mut backend, &TransactionRequest::Remove),
        Err(TransactionError::LeaseBusy { .. })
    ));
    assert!(staging.join("backups/driver").exists());
    drop(guard);
    execute(&mut backend, &TransactionRequest::Remove).unwrap();
    assert!(!staging.exists());
    assert!(OwnedTarget::all().iter().all(|target| {
        !fixture
            .path
            .join(target.path().trim_start_matches('/'))
            .exists()
    }));
}

#[test]
fn rejected_stage_never_mutates_installed_targets_and_releases_leases() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(false, false);
    backend.service.stage_failure = Some(StageFailure::Reject);
    assert!(matches!(
        execute(&mut backend, &TransactionRequest::remove()),
        Err(TransactionError::Stage(CoordinatedError::Service(_)))
    ));
    assert!(!backend.filesystem.layout().staging_root().exists());
    assert_eq!(
        fs::read(fixture.path.join(DRIVER_PATH.trim_start_matches('/'))).unwrap(),
        b"original"
    );
    LifecycleDirectory::open(
        fixture
            .path
            .join(LIFECYCLE_DIRECTORY.trim_start_matches('/')),
    )
    .unwrap()
    .acquire()
    .unwrap();
}

#[test]
fn rejected_stage_reports_both_validation_and_cleanup_failures() {
    let fixture = Fixture::new();
    let mut backend = fixture.backend(false, false);
    backend.service.stage_failure = Some(StageFailure::RejectWithCleanupFailure);
    assert!(matches!(
        execute(&mut backend, &TransactionRequest::remove()),
        Err(TransactionError::Stage(
            CoordinatedError::StageCleanup { .. }
        ))
    ));
    assert!(
        backend
            .filesystem
            .layout()
            .staging_root()
            .with_extension("displaced")
            .is_dir()
    );
    assert_eq!(
        fs::read(fixture.path.join(DRIVER_PATH.trim_start_matches('/'))).unwrap(),
        b"original"
    );
}

#[test]
fn file_recovery_restores_persistent_driver_registration_before_cleanup() {
    let fixture = Fixture::new();
    // Drain may have stopped the Driver before a file commit failed.
    let mut backend = fixture.backend(false, false);
    let request = TransactionRequest::remove();
    let preflight = backend.preflight(&request).unwrap();
    let staged = backend.stage(&request, &preflight).unwrap();
    backend
        .rollback(&request, &preflight, &staged, None)
        .unwrap();
    backend.cleanup(&request, &preflight, staged).unwrap();
}
