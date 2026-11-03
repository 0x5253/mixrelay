use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

struct Fixture {
    _root: mixrelay_test_support::FixtureDir,
    root: PathBuf,
    sources: InstallSources,
}

impl Fixture {
    fn new() -> Self {
        let root_owner = mixrelay_test_support::FixtureDir::new("macos-release").unwrap();
        let root = root_owner.path().to_path_buf();
        let sources = InstallSources::new(
            root.join("app"),
            root.join("hal"),
            (root.join("driver"), root.join("launchd")),
            (root.join("remover"), root.join("removal-launchd")),
            (root.join("engine"), root.join("engine-launchd")),
        );
        for (bundle, identifier, executable) in [
            (sources.app(), "io.mixrelay.desktop", "mixrelay-desktop"),
            (sources.hal(), "io.mixrelay.audio.driver", "MixRelayHAL"),
            (
                sources.remover(),
                "io.mixrelay.driver.uninstaller",
                "mixrelay-driver-uninstaller",
            ),
        ] {
            fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
            fs::write(bundle.join("Contents/Info.plist"), format!("<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{identifier}</string><key>CFBundleExecutable</key><string>{executable}</string></dict></plist>")).unwrap();
            write_universal(&bundle.join("Contents/MacOS").join(executable), false);
        }
        write_universal(
            &sources
                .remover()
                .join("Contents/Helpers/mixrelay-macos-installer"),
            true,
        );
        write_universal(sources.driver(), true);
        write_universal(sources.runtime_engine(), true);
        fs::write(
            sources.launchd(),
            include_bytes!("../../../../../packaging/macos/io.mixrelay.audio.driver.plist.in"),
        )
        .unwrap();
        fs::write(
            sources.runtime_launchd(),
            include_bytes!("../../../../../packaging/macos/io.mixrelay.audio.engine.plist.in"),
        )
        .unwrap();
        #[cfg(target_os = "macos")]
        fs::write(
            sources.removal_launchd(),
            crate::removal::removal_launchd_template(&verifier().team),
        )
        .unwrap();
        #[cfg(not(target_os = "macos"))]
        fs::write(sources.removal_launchd(), b"fixture").unwrap();
        Self {
            _root: root_owner,
            root,
            sources,
        }
    }
}

fn write_universal(path: &Path, wide: bool) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, crate::universal::tests::fixture(wide)).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn verifier() -> ReleasePayloadVerifier {
    ReleasePayloadVerifier::new(
        SigningTeam::new("ABCD123456".into()).unwrap(),
        ServiceAccount::new("listener".into(), "staff".into(), "/Users/listener".into()).unwrap(),
    )
}

fn prepare_installed_target(
    fixture: &Fixture,
    verifier: &ReleasePayloadVerifier,
    target: OwnedTarget,
) {
    let path = fixture.root.join(target.path().trim_start_matches('/'));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    match target {
        OwnedTarget::App => fs::rename(fixture.sources.app(), &path).unwrap(),
        OwnedTarget::Hal => fs::rename(fixture.sources.hal(), &path).unwrap(),
        OwnedTarget::RuntimeEngine => {
            fs::write(&path, crate::universal::tests::fixture(true)).unwrap();
        }
        OwnedTarget::RuntimeLaunchd => {
            crate::runtime_definition::for_account(&verifier.account)
                .unwrap()
                .to_file_xml(&path)
                .unwrap();
        }
        OwnedTarget::Driver => fs::rename(fixture.sources.driver(), &path).unwrap(),
        OwnedTarget::Launchd => {
            let bytes = fs::read(fixture.sources.launchd()).unwrap();
            fs::write(
                &path,
                LaunchdConfiguration::from_template(&bytes, &verifier.account)
                    .unwrap()
                    .to_xml()
                    .unwrap(),
            )
            .unwrap();
        }
        OwnedTarget::Uninstaller => fs::rename(fixture.sources.remover(), &path).unwrap(),
        OwnedTarget::RemovalLaunchd => {
            #[cfg(target_os = "macos")]
            fs::write(
                path,
                crate::removal::removal_launchd_template(&verifier.team),
            )
            .unwrap();
            #[cfg(not(target_os = "macos"))]
            fs::write(path, b"fixture").unwrap();
        }
    }
}

#[test]
fn removal_verifies_present_targets_in_all_partial_installations() {
    for mask in 0u16..(1 << OwnedTarget::all().len()) {
        let fixture = Fixture::new();
        let verifier = verifier();
        for (index, target) in OwnedTarget::all().into_iter().enumerate() {
            if mask & (1 << index) == 0 {
                continue;
            }
            prepare_installed_target(&fixture, &verifier, target);
        }
        let mut checked = Vec::new();
        let definition = verifier
            .verify_ownership_with(
                &FilesystemLayout::new(&fixture.root).unwrap(),
                |path, requirement, role, _| {
                    assert!(requirement.contains("anchor apple generic"));
                    assert!(requirement.contains("ABCD123456"));
                    checked.push((path.to_path_buf(), role));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(definition.is_some(), mask & (1 << 3) != 0);
        let expected = usize::from(mask & 1 != 0)
            + usize::from(mask & 2 != 0)
            + usize::from(mask & 4 != 0)
            + 2 * usize::from(mask & 16 != 0)
            + usize::from(mask & 64 != 0);
        assert_eq!(checked.len(), expected);
        for (path, _) in checked {
            assert!(path.exists());
        }
    }
}

#[test]
fn removal_does_not_ignore_present_invalid_code_or_configuration() {
    let fixture = Fixture::new();
    let layout = FilesystemLayout::new(&fixture.root).unwrap();
    let path = fixture
        .root
        .join(OwnedTarget::Driver.path().trim_start_matches('/'));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::rename(fixture.sources.driver(), &path).unwrap();
    assert!(matches!(
        verifier().verify_ownership_with(&layout, |path, _, _, _| Err(ReleaseError::Signature {
            path: path.to_path_buf(),
            status: "wrong publisher".into()
        })),
        Err(ReleaseError::Signature { .. })
    ));
    fs::remove_file(&path).unwrap();
    let path = fixture
        .root
        .join(OwnedTarget::Launchd.path().trim_start_matches('/'));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"invalid configuration").unwrap();
    assert!(
        verifier()
            .verify_ownership_with(&layout, |_, _, _, _| Ok(()))
            .is_err()
    );
}

#[test]
fn binds_every_code_identity_and_team_without_shell_interpolation() {
    let fixture = Fixture::new();
    let mut checks = Vec::new();
    verifier()
        .verify_with(&fixture.sources, true, |path, requirement, role, _| {
            checks.push((path.to_path_buf(), requirement.to_owned(), role));
            Ok(())
        })
        .unwrap();
    assert_eq!(checks.len(), 6);
    for ((_, requirement, role), (identifier, expected_role)) in checks.iter().zip([
        ("io.mixrelay.service", ArtifactRole::Engine),
        ("io.mixrelay.driver.uninstaller", ArtifactRole::Uninstaller),
        ("mixrelay-macos-installer", ArtifactRole::Installer),
        ("io.mixrelay.desktop", ArtifactRole::Application),
        ("io.mixrelay.audio.driver", ArtifactRole::Hal),
        ("mixrelay-driver-service", ArtifactRole::Driver),
    ]) {
        assert!(requirement.contains(&format!("identifier \"{identifier}\"")));
        assert!(requirement.contains("anchor apple generic"));
        assert!(requirement.contains("ABCD123456"));
        assert!(requirement.contains("1.2.840.113635.100.6.1.13"));
        assert_eq!(*role, expected_role);
    }
}

#[test]
fn invalid_team_and_template_fail_before_native_code_checks() {
    for team in ["", "short", "abcdefghij", "ABCD12345\"", "ABCD12345;"] {
        assert!(SigningTeam::new(team.into()).is_err());
    }
    let fixture = Fixture::new();
    fs::write(fixture.sources.launchd(), b"not a plist").unwrap();
    assert!(
        verifier()
            .verify_with(&fixture.sources, true, |_, _, _, _| panic!(
                "signature should not run"
            ))
            .is_err()
    );
}

#[test]
fn relative_paths_cannot_be_interpreted_as_codesign_options() {
    let sources = InstallSources::new(
        "--help",
        "/hal",
        ("/driver", "/launchd"),
        ("/remover", "/removal-launchd"),
        ("/engine", "/engine-launchd"),
    );
    assert!(matches!(
        verifier().verify_with(&sources, true, |_, _, _, _| panic!(
            "must reject before command"
        )),
        Err(ReleaseError::SourcePath(_))
    ));
}

#[test]
fn signature_failure_short_circuits_the_remaining_payload() {
    let fixture = Fixture::new();
    let mut calls = 0;
    let result = verifier().verify_with(&fixture.sources, true, |path, _, _, _| {
        calls += 1;
        Err(ReleaseError::Signature {
            path: path.to_path_buf(),
            status: "rejected".into(),
        })
    });
    assert!(matches!(result, Err(ReleaseError::Signature { .. })));
    assert_eq!(calls, 1);
}

#[test]
fn wrong_bundle_identity_and_thin_binary_are_rejected_after_signature() {
    let fixture = Fixture::new();
    let info = fixture.sources.hal().join("Contents/Info.plist");
    let original = fs::read_to_string(&info).unwrap();
    fs::write(
        &info,
        original.replace("io.mixrelay.audio.driver", "io.unrelated.driver"),
    )
    .unwrap();
    assert!(matches!(
        verifier().verify_with(&fixture.sources, true, |_, _, _, _| Ok(())),
        Err(ReleaseError::BundleIdentity(_))
    ));
    fs::write(info, original).unwrap();
    fs::write(fixture.sources.driver(), b"not universal").unwrap();
    assert!(matches!(
        verifier().verify_with(&fixture.sources, true, |_, _, _, _| Ok(())),
        Err(ReleaseError::Universal { .. })
    ));
}

#[test]
fn installer_helper_uses_fixed_team_identity_and_entitlement_free_role() {
    let fixture = Fixture::new();
    let mut observed = None;
    verifier()
        .verify_installer_executable_with(
            fixture.sources.driver(),
            VerificationPolicy::Launch,
            |path, requirement, role, _| {
                observed = Some((path.to_path_buf(), requirement.to_owned(), role));
                Ok(())
            },
        )
        .unwrap();
    let (path, requirement, role) = observed.expect("signature verifier should be called");
    assert_eq!(path, fixture.sources.driver());
    assert!(requirement.contains("identifier \"mixrelay-macos-installer\""));
    assert!(requirement.contains("ABCD123456"));
    assert_eq!(role, ArtifactRole::Installer);
}

#[test]
fn installer_helper_rejects_relative_paths_before_signature_verification() {
    assert!(matches!(
        verifier().verify_installer_executable_with(
            Path::new("helper"),
            VerificationPolicy::Launch,
            |_, _, _, _| { panic!("relative path must be rejected first") }
        ),
        Err(ReleaseError::SourcePath(_))
    ));
}

#[test]
fn configuration_reader_rejects_symlinks_directories_and_oversized_files() {
    let fixture = Fixture::new();
    assert!(read_configuration(&fixture.root, 128).is_err());
    let alias = fixture.root.join("alias");
    symlink(fixture.sources.launchd(), &alias).unwrap();
    assert!(read_configuration(&alias, 16 * 1024).is_err());
    assert!(read_configuration(fixture.sources.launchd(), 8).is_err());
    assert!(read_configuration(fixture.sources.launchd(), 16 * 1024).is_ok());
}

#[test]
fn reopening_needs_only_the_closed_apps_dependencies_and_valid_present_targets() {
    let runtime = [
        OwnedTarget::App,
        OwnedTarget::Driver,
        OwnedTarget::Hal,
        OwnedTarget::Launchd,
        OwnedTarget::RuntimeEngine,
        OwnedTarget::RuntimeLaunchd,
    ];
    let fixture = Fixture::new();
    let verifier = verifier();
    let layout = FilesystemLayout::new(&fixture.root).unwrap();
    // A failed repair of an installation that lacks the remover rolls back to it.
    for target in runtime {
        prepare_installed_target(&fixture, &verifier, target);
    }
    let mut checked = Vec::new();
    verifier
        .verify_installed_with(&layout, &runtime, |path, _, role, _| {
            checked.push((path.to_path_buf(), role));
            Ok(())
        })
        .unwrap();
    assert_eq!(checked.len(), 4);
    assert!(
        checked
            .iter()
            .all(|(path, _)| path.starts_with(&fixture.root))
    );

    // Present code outside the closed apps' dependencies is still verified.
    prepare_installed_target(&fixture, &verifier, OwnedTarget::Uninstaller);
    assert!(matches!(
        verifier.verify_installed_with(&layout, &runtime, |path, _, role, _| {
            if role == ArtifactRole::Uninstaller {
                return Err(ReleaseError::Signature {
                    path: path.to_path_buf(),
                    status: "wrong publisher".into(),
                });
            }
            Ok(())
        }),
        Err(ReleaseError::Signature { .. })
    ));

    // A missing dependency blocks reopening only the app that needs it.
    fs::remove_file(
        fixture
            .root
            .join(OwnedTarget::Driver.path().trim_start_matches('/')),
    )
    .unwrap();
    assert!(matches!(
        verifier.verify_installed_with(&layout, &runtime, |_, _, _, _| Ok(())),
        Err(ReleaseError::Io { .. })
    ));
    let graph = [
        OwnedTarget::App,
        OwnedTarget::RuntimeEngine,
        OwnedTarget::RuntimeLaunchd,
    ];
    verifier
        .verify_installed_with(&layout, &graph, |_, _, _, _| Ok(()))
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn stage_verification_checks_private_copies_even_when_sources_are_removed() {
    use crate::{FilesystemAdapter, FilesystemLayout, LIFECYCLE_DIRECTORY, TransactionRequest};
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
    fs::remove_dir_all(fixture.sources.app()).unwrap();
    fs::remove_dir_all(fixture.sources.hal()).unwrap();
    fs::remove_file(fixture.sources.driver()).unwrap();
    fs::remove_file(fixture.sources.launchd()).unwrap();
    fs::remove_dir_all(fixture.sources.remover()).unwrap();
    fs::remove_file(fixture.sources.removal_launchd()).unwrap();
    fs::remove_file(fixture.sources.runtime_engine()).unwrap();
    fs::remove_file(fixture.sources.runtime_launchd()).unwrap();
    let mut checked = Vec::new();
    verifier()
        .verify_staged_with(&staged, |path, _, _, _| {
            assert!(path.exists(), "{} must be a staged copy", path.display());
            checked.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();
    let remover = staged.payload_path(OwnedTarget::Uninstaller);
    assert_eq!(
        checked,
        [
            staged.payload_path(OwnedTarget::RuntimeEngine),
            remover.clone(),
            remover.join("Contents/Helpers/mixrelay-macos-installer"),
            staged.payload_path(OwnedTarget::App),
            staged.payload_path(OwnedTarget::Hal),
            staged.payload_path(OwnedTarget::Driver),
        ]
    );
    adapter.cleanup(&staged).unwrap();
}

mod ownership;

mod recovery;
