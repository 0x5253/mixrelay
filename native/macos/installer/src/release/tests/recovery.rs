//! Real file rollback with injected signing metadata and service activation.

use super::{
    ownership::{check_metadata, signed_metadata, wrong_entitlements},
    *,
};
use crate::{
    CoordinatedBackend, LIFECYCLE_DIRECTORY, OwnedInstallationSnapshot, RecoveryStatus,
    ServiceLifecycle, TransactionError, TransactionRequest, execute,
};
use std::{cell::RefCell, io, rc::Rc};

fn signature(
    path: &Path,
    requirement: &str,
    role: ArtifactRole,
    policy: VerificationPolicy,
) -> Result<(), ReleaseError> {
    assert!(requirement.contains("certificate leaf[subject.OU] = \"ABCD123456\""));
    let executable = match role {
        ArtifactRole::Application => path.join("Contents/MacOS/mixrelay-desktop"),
        ArtifactRole::Hal => path.join("Contents/MacOS/MixRelayHAL"),
        ArtifactRole::Uninstaller => path.join("Contents/MacOS/mixrelay-driver-uninstaller"),
        _ => path.to_path_buf(),
    };
    let bytes = fs::read(executable).unwrap();
    let mut slices = signed_metadata(role);
    if bytes.ends_with(b"previous") {
        for (_, metadata) in &mut slices {
            metadata.entitlements = [("previous.release.entitlement".into(), Some(true))].into();
            metadata.entitlement_count = 1;
        }
    }
    check_metadata(path, role, policy, &slices)
}

fn setup() -> (Fixture, FilesystemLayout) {
    let (fixture, verifier) = (Fixture::new(), verifier());
    for target in OwnedTarget::all() {
        prepare_installed_target(&fixture, &verifier, target);
    }
    let layout = FilesystemLayout::new(&fixture.root).unwrap();
    // All old roles have different entitlements, including app, engine and helper.
    for (target, relative) in [
        (OwnedTarget::App, "Contents/MacOS/mixrelay-desktop"),
        (OwnedTarget::Hal, "Contents/MacOS/MixRelayHAL"),
        (
            OwnedTarget::Uninstaller,
            "Contents/MacOS/mixrelay-driver-uninstaller",
        ),
        (
            OwnedTarget::Uninstaller,
            "Contents/Helpers/mixrelay-macos-installer",
        ),
    ] {
        let path = layout.target_path(target).join(relative);
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"previous");
        fs::write(path, bytes).unwrap();
    }
    for target in [OwnedTarget::RuntimeEngine, OwnedTarget::Driver] {
        let path = layout.target_path(target);
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"previous");
        fs::write(path, bytes).unwrap();
    }
    let lifecycle = fixture
        .root
        .join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
    fs::create_dir_all(&lifecycle).unwrap();
    fs::set_permissions(lifecycle, fs::Permissions::from_mode(0o700)).unwrap();
    (fixture, layout)
}

struct Services {
    layout: FilesystemLayout,
    verifier: ReleasePayloadVerifier,
    active: Rc<RefCell<Vec<&'static str>>>,
    fail_readiness: bool,
    corrupt_new_engine: bool,
}

impl ServiceLifecycle for Services {
    type Snapshot = OwnedInstallationSnapshot;
    type Registration = ();
    type Error = ReleaseError;

    fn inspect(
        &mut self,
        _: &TransactionRequest,
        layout: &FilesystemLayout,
    ) -> Result<Self::Snapshot, Self::Error> {
        self.verifier.snapshot_owned_with(layout, signature)
    }

    fn validate_stage(
        &mut self,
        _: &TransactionRequest,
        staged: &FilesystemStaged,
    ) -> Result<(), Self::Error> {
        let definition = self.verifier.verify_staged_with(staged, signature)?;
        fs::write(
            staged.payload_path(OwnedTarget::Launchd),
            definition.to_xml()?,
        )
        .unwrap();
        crate::runtime_definition::for_account(&self.verifier.account)
            .unwrap()
            .to_file_xml(staged.payload_path(OwnedTarget::RuntimeLaunchd))
            .unwrap();
        Ok(())
    }

    fn drain(&mut self, _: &Self::Snapshot) -> Result<(), Self::Error> {
        self.active.borrow_mut().clear();
        Ok(())
    }

    fn register(&mut self, _: &TransactionRequest) -> Result<(), Self::Error> {
        self.verifier.verify_installed_with(
            &self.layout,
            &OwnedTarget::all(),
            |path, requirement, role, policy| {
                if self.corrupt_new_engine && role == ArtifactRole::Engine {
                    wrong_entitlements(role)(path, requirement, role, policy)
                } else {
                    signature(path, requirement, role, policy)
                }
            },
        )?;
        self.active.borrow_mut().push("new engine");
        if self.fail_readiness {
            return Err(ReleaseError::Io {
                path: self.layout.target_path(OwnedTarget::Driver),
                source: io::Error::other("injected readiness failure after replacement"),
            });
        }
        self.active
            .borrow_mut()
            .extend(["new Driver", "new remover"]);
        Ok(())
    }

    fn prepare_restore(&mut self, _: &Self::Snapshot) -> Result<(), Self::Error> {
        self.active.borrow_mut().clear();
        Ok(())
    }

    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), Self::Error> {
        self.verifier.verify_restored_with(
            &self.layout,
            &OwnedTarget::all(),
            snapshot,
            signature,
        )?;
        self.active
            .borrow_mut()
            .extend(["previous remover", "previous engine", "previous Driver"]);
        Ok(())
    }
}

#[test]
fn failed_upgrade_reactivates_previous_services_with_different_entitlements() {
    let (_old, layout) = setup();
    let new = Fixture::new();
    let active = Rc::new(RefCell::new(vec![
        "previous Driver",
        "previous engine",
        "previous remover",
    ]));
    let service = Services {
        layout: layout.clone(),
        verifier: verifier(),
        active: active.clone(),
        fail_readiness: true,
        corrupt_new_engine: false,
    };
    let mut backend = CoordinatedBackend::new(layout.clone(), service);
    let result = execute(&mut backend, &TransactionRequest::install(new.sources));
    assert!(
        matches!(
            result,
            Err(TransactionError::MutationRecovery {
                recovery: RecoveryStatus::Restored { .. },
                ..
            })
        ),
        "{result:?}"
    );
    assert_eq!(
        *active.borrow(),
        ["previous remover", "previous engine", "previous Driver"]
    );
    assert!(!layout.staging_root().exists());
}

#[test]
fn new_installation_enforces_launch_policy_before_registration() {
    for corrupt_new_engine in [false, true] {
        let (_old, layout) = setup();
        let new = Fixture::new();
        let active = Rc::new(RefCell::new(Vec::new()));
        let service = Services {
            layout: layout.clone(),
            verifier: verifier(),
            active: active.clone(),
            fail_readiness: false,
            corrupt_new_engine,
        };
        let mut backend = CoordinatedBackend::new(layout, service);
        let result = execute(&mut backend, &TransactionRequest::install(new.sources));
        if corrupt_new_engine {
            assert!(
                matches!(
                    result,
                    Err(TransactionError::MutationRecovery {
                        cause: crate::CoordinatedError::Service(ReleaseError::SignaturePolicy {
                            reason: "embedded entitlements differ from the artifact allowlist",
                            ..
                        }),
                        recovery: RecoveryStatus::Restored { .. },
                        ..
                    })
                ),
                "{result:?}"
            );
            assert_eq!(
                *active.borrow(),
                ["previous remover", "previous engine", "previous Driver"]
            );
        } else {
            result.unwrap();
            assert_eq!(
                *active.borrow(),
                ["new engine", "new Driver", "new remover"]
            );
        }
    }
}

#[test]
fn restored_code_still_rejects_other_team_and_unsigned_signatures() {
    let (_old, layout) = setup();
    let verifier = verifier();
    let snapshot = verifier.snapshot_owned_with(&layout, signature).unwrap();
    for rejected in [
        ArtifactRole::Application,
        ArtifactRole::Engine,
        ArtifactRole::Driver,
        ArtifactRole::Hal,
        ArtifactRole::Uninstaller,
        ArtifactRole::Installer,
    ] {
        for status in ["other team", "unsigned"] {
            let mut activated = false;
            let result = verifier
                .verify_restored_with(
                    &layout,
                    &[],
                    &snapshot,
                    |path, requirement, role, policy| {
                        if role == rejected {
                            return Err(ReleaseError::Signature {
                                path: path.to_path_buf(),
                                status: status.into(),
                            });
                        }
                        signature(path, requirement, role, policy)
                    },
                )
                .map(|()| activated = true);
            assert!(
                matches!(result, Err(ReleaseError::Signature { status: actual, .. }) if actual == status),
                "{rejected:?}: {status}"
            );
            assert!(!activated);
        }
    }
}

#[test]
fn recovery_rejects_a_different_owned_installation_and_snapshot_binding() {
    let (_old, layout) = setup();
    let verifier = verifier();
    let snapshot = verifier.snapshot_owned_with(&layout, signature).unwrap();
    let other = ReleasePayloadVerifier::new(
        SigningTeam::new("OTHER12345".into()).unwrap(),
        verifier.account.clone(),
    );
    assert!(matches!(
        other.verify_restored_with(&layout, &[], &snapshot, signature),
        Err(ReleaseError::SnapshotIdentity)
    ));
    let engine = layout.target_path(OwnedTarget::RuntimeEngine);
    fs::rename(&engine, engine.with_extension("previous")).unwrap();
    write_universal(&engine, true);
    assert!(matches!(
        verifier.verify_restored_with(&layout, &[], &snapshot, signature),
        Err(ReleaseError::Filesystem(
            crate::FilesystemError::ConcurrentTargetChange {
                target: OwnedTarget::RuntimeEngine
            }
        ))
    ));
}

#[test]
fn recovery_rejects_replaced_executable_inside_the_original_app_bundle() {
    let (_old, layout) = setup();
    let verifier = verifier();
    let snapshot = verifier.snapshot_owned_with(&layout, signature).unwrap();
    let executable = layout
        .target_path(OwnedTarget::App)
        .join("Contents/MacOS/mixrelay-desktop");
    fs::rename(&executable, executable.with_extension("previous")).unwrap();
    write_universal(&executable, true);
    assert!(matches!(
        verifier.verify_restored_with(&layout, &[], &snapshot, signature),
        Err(ReleaseError::SnapshotContents)
    ));
}

#[test]
fn previous_app_recovery_requires_its_dependencies_and_allows_a_missing_remover() {
    let (_old, layout) = setup();
    fs::remove_dir_all(layout.target_path(OwnedTarget::Uninstaller)).unwrap();
    fs::remove_file(layout.target_path(OwnedTarget::RemovalLaunchd)).unwrap();
    let verifier = verifier();
    let snapshot = verifier.snapshot_owned_with(&layout, signature).unwrap();
    let graph = [
        OwnedTarget::App,
        OwnedTarget::RuntimeEngine,
        OwnedTarget::RuntimeLaunchd,
    ];
    verifier
        .verify_restored_with(&layout, &graph, &snapshot, signature)
        .unwrap();
    assert!(matches!(
        verifier.verify_restored_with(&layout, &OwnedTarget::all(), &snapshot, signature),
        Err(ReleaseError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound
    ));
}
