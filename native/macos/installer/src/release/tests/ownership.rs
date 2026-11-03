//! Installed ownership and launch policy regression scenarios.

use super::*;
use crate::release::policy::{
    Architecture, SignatureMetadata, expected_entitlements, verify_slices,
};

pub(super) fn signed_metadata(role: ArtifactRole) -> Vec<(Architecture, SignatureMetadata)> {
    Architecture::ALL
        .into_iter()
        .map(|architecture| {
            (
                architecture,
                SignatureMetadata {
                    flags: 0x0001_0000, // Hardened Runtime code-signature flag.
                    has_secure_timestamp: true,
                    entitlement_count: expected_entitlements(role).len(),
                    entitlements: expected_entitlements(role)
                        .iter()
                        .map(|(key, value)| ((*key).into(), Some(*value)))
                        .collect(),
                },
            )
        })
        .collect()
}

#[test]
fn installed_ownership_accepts_app_without_current_entitlements() {
    for entitlements in [
        std::collections::BTreeMap::new(),
        [(
            "com.apple.security.files.user-selected.read-only".into(),
            Some(true),
        )]
        .into(),
    ] {
        let fixture = Fixture::new();
        let verifier = verifier();
        prepare_installed_target(&fixture, &verifier, OwnedTarget::App);
        let layout = FilesystemLayout::new(&fixture.root).unwrap();
        verifier
            .verify_ownership_with(&layout, |path, _, role, policy| {
                let mut slices = signed_metadata(role);
                for (_, metadata) in &mut slices {
                    metadata.entitlement_count = entitlements.len();
                    metadata.entitlements.clone_from(&entitlements);
                }
                check_metadata(path, role, policy, &slices)
            })
            .expect("owned installed app must be replaceable and removable");
    }
}

pub(super) fn check_metadata(
    path: &Path,
    role: ArtifactRole,
    policy: VerificationPolicy,
    slices: &[(Architecture, SignatureMetadata)],
) -> Result<(), ReleaseError> {
    verify_slices(role, policy, slices)
        .map_err(|(architecture, violation)| policy_error(path, architecture, violation))
}

const ROLES: [ArtifactRole; 6] = [
    ArtifactRole::Application,
    ArtifactRole::Engine,
    ArtifactRole::Hal,
    ArtifactRole::Driver,
    ArtifactRole::Uninstaller,
    ArtifactRole::Installer,
];

fn installed_fixture() -> (Fixture, ReleasePayloadVerifier, FilesystemLayout) {
    let fixture = Fixture::new();
    let verifier = verifier();
    for target in OwnedTarget::all() {
        prepare_installed_target(&fixture, &verifier, target);
    }
    let layout = FilesystemLayout::new(&fixture.root).unwrap();
    (fixture, verifier, layout)
}

#[test]
fn replacement_and_removal_ownership_accept_different_entitlements_for_every_role() {
    let (_fixture, verifier, layout) = installed_fixture();
    let mut checked = Vec::new();
    verifier
        .verify_ownership_with(&layout, |path, requirement, role, policy| {
            assert!(requirement.contains("certificate leaf[subject.OU] = \"ABCD123456\""));
            let mut slices = signed_metadata(role);
            for (_, metadata) in &mut slices {
                metadata.entitlements =
                    [("com.apple.security.cs.allow-jit".into(), Some(true))].into();
                metadata.entitlement_count = 1;
            }
            checked.push(role);
            check_metadata(path, role, policy, &slices)
        })
        .unwrap();
    assert_eq!(checked.len(), ROLES.len());
    assert!(ROLES.iter().all(|role| checked.contains(role)));
}

#[test]
fn ownership_rejects_other_team_and_unsigned_code_for_every_role() {
    let (_fixture, verifier, layout) = installed_fixture();
    for rejected_role in ROLES {
        for status in ["other team", "unsigned"] {
            let result = verifier.verify_ownership_with(&layout, |path, _, role, policy| {
                if role == rejected_role {
                    return Err(ReleaseError::Signature {
                        path: path.to_path_buf(),
                        status: status.into(),
                    });
                }
                check_metadata(path, role, policy, &signed_metadata(role))
            });
            assert!(
                matches!(result, Err(ReleaseError::Signature {status: actual, ..}) if actual == status),
                "{rejected_role:?}: {status}"
            );
        }
    }
}

#[test]
fn ownership_requires_hardened_runtime_timestamp_and_both_architectures_for_every_role() {
    let (_fixture, verifier, layout) = installed_fixture();
    for rejected_role in ROLES {
        for defect in [
            PolicyViolation::HardenedRuntime,
            PolicyViolation::SecureTimestamp,
            PolicyViolation::ArchitectureSet,
        ] {
            let result = verifier.verify_ownership_with(&layout, |path, _, role, policy| {
                let mut slices = signed_metadata(role);
                if role == rejected_role {
                    match defect {
                        PolicyViolation::HardenedRuntime => slices[1].1.flags = 0,
                        PolicyViolation::SecureTimestamp => {
                            slices[1].1.has_secure_timestamp = false;
                        }
                        PolicyViolation::ArchitectureSet => {
                            slices.pop();
                        }
                        PolicyViolation::Entitlements => unreachable!(),
                    }
                }
                check_metadata(path, role, policy, &slices)
            });
            let Err(ReleaseError::SignaturePolicy { reason, .. }) = result else {
                panic!("{rejected_role:?} must reject {defect:?}");
            };
            let ReleaseError::SignaturePolicy {
                reason: expected, ..
            } = policy_error(Path::new("fixture"), Architecture::X86_64, defect)
            else {
                unreachable!()
            };
            assert_eq!(reason, expected);
        }
    }
}

pub(super) fn wrong_entitlements(
    rejected_role: ArtifactRole,
) -> impl FnMut(&Path, &str, ArtifactRole, VerificationPolicy) -> Result<(), ReleaseError> {
    move |path, _, role, policy| {
        let mut slices = signed_metadata(role);
        if role == rejected_role {
            // No entitlements for app/engine; an extra entitlement for all other roles.
            for (_, metadata) in &mut slices {
                metadata.entitlements =
                    if matches!(role, ArtifactRole::Application | ArtifactRole::Engine) {
                        std::collections::BTreeMap::new()
                    } else {
                        [("com.apple.security.cs.allow-jit".into(), Some(true))].into()
                    };
                metadata.entitlement_count = metadata.entitlements.len();
            }
        }
        check_metadata(path, role, policy, &slices)
    }
}

#[test]
fn source_payload_requires_exact_entitlements_for_every_role() {
    let fixture = Fixture::new();
    for role in ROLES {
        assert!(
            matches!(
                verifier().verify_with(&fixture.sources, true, wrong_entitlements(role)),
                Err(ReleaseError::SignaturePolicy {
                    reason: "embedded entitlements differ from the artifact allowlist",
                    ..
                })
            ),
            "{role:?}"
        );
    }
}

#[test]
fn installed_result_requires_exact_entitlements_before_relaunch_for_every_role() {
    let (_fixture, verifier, layout) = installed_fixture();
    verifier
        .verify_installed_with(&layout, &OwnedTarget::all(), |path, _, role, policy| {
            check_metadata(path, role, policy, &signed_metadata(role))
        })
        .unwrap();
    for role in ROLES {
        assert!(
            matches!(
                verifier.verify_installed_with(
                    &layout,
                    &OwnedTarget::all(),
                    wrong_entitlements(role)
                ),
                Err(ReleaseError::SignaturePolicy {
                    reason: "embedded entitlements differ from the artifact allowlist",
                    ..
                })
            ),
            "{role:?}"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn staged_payload_requires_exact_entitlements_for_every_role() {
    use crate::{FilesystemAdapter, LIFECYCLE_DIRECTORY, TransactionRequest};
    let fixture = Fixture::new();
    fs::create_dir_all(
        fixture
            .root
            .join(LIFECYCLE_DIRECTORY.trim_start_matches('/')),
    )
    .unwrap();
    let adapter = FilesystemAdapter::new(FilesystemLayout::new(&fixture.root).unwrap());
    let request = TransactionRequest::install(fixture.sources.clone());
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    for role in ROLES {
        assert!(
            matches!(
                verifier().verify_staged_with(&staged, wrong_entitlements(role)),
                Err(ReleaseError::SignaturePolicy {
                    reason: "embedded entitlements differ from the artifact allowlist",
                    ..
                })
            ),
            "{role:?}"
        );
    }
    adapter.cleanup(&staged).unwrap();
}

#[test]
fn ownership_rejects_foreign_bundle_identifier_or_executable() {
    for (expected, foreign) in [
        ("io.mixrelay.desktop", "io.foreign.desktop"),
        ("mixrelay-desktop", "foreign-desktop"),
    ] {
        let fixture = Fixture::new();
        let verifier = verifier();
        prepare_installed_target(&fixture, &verifier, OwnedTarget::App);
        let info = fixture
            .root
            .join(OwnedTarget::App.path().trim_start_matches('/'))
            .join("Contents/Info.plist");
        let contents = fs::read_to_string(&info).unwrap();
        fs::write(&info, contents.replace(expected, foreign)).unwrap();
        assert!(matches!(
            verifier.verify_ownership_with(
                &FilesystemLayout::new(&fixture.root).unwrap(),
                |_, _, _, _| panic!("foreign bundle must fail before signature inspection"),
            ),
            Err(ReleaseError::BundleIdentity(_))
        ));
    }
}
