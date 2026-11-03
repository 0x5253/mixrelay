//! Actual filesystem tests for the unprivileged adapter.

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use super::*;
use crate::{InstallSources, OperationKind};

mod races;

struct Fixture {
    _root: mixrelay_test_support::FixtureDir,
    root: PathBuf,
    app: PathBuf,
    hal: PathBuf,
    driver: PathBuf,
    launchd: PathBuf,
    remover: PathBuf,
    removal_launchd: PathBuf,
    runtime_engine: PathBuf,
    runtime_launchd: PathBuf,
    user_data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root_owner = mixrelay_test_support::FixtureDir::new("macos-filesystem")
            .expect("fixture root creates");
        let root = root_owner.path().to_path_buf();
        let lifecycle = root.join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
        fs::create_dir_all(&lifecycle).expect("lifecycle creates");
        let user_data = root.join("Users/fixture/Library/Application Support/MixRelay");
        fs::create_dir_all(&user_data).expect("user data creates");
        let fixture = Self {
            _root: root_owner,
            app: root.join("sources/MixRelay.app"),
            hal: root.join("sources/MixRelayHAL.driver"),
            driver: root.join("sources/mixrelay-driver-service"),
            launchd: root.join("sources/io.mixrelay.audio.driver.plist"),
            remover: root.join("sources/Uninstall MixRelay.app"),
            removal_launchd: root.join("sources/io.mixrelay.driver.removal.plist"),
            runtime_engine: root.join("sources/mixrelay-service"),
            runtime_launchd: root.join("sources/io.mixrelay.audio.engine.plist"),
            user_data: user_data.join("graphs.json"),
            root,
        };
        fixture.create_sources();
        fixture.create_user_data();
        fixture
    }

    fn create_sources(&self) {
        let app_executable = self.app.join("Contents/MacOS/mixrelay-desktop");
        let hal_executable = self.hal.join("Contents/MacOS/MixRelayHAL");
        write_executable(&app_executable, "new-app");
        write_executable(&hal_executable, "new-hal");
        write_file(&self.app.join("Contents/Info.plist"), "app-info");
        write_file(&self.hal.join("Contents/Info.plist"), "hal-info");
        fs::create_dir_all(self.app.join("Contents/Resources/empty"))
            .expect("empty app directory creates");
        fs::create_dir_all(self.hal.join("Contents/Resources/empty"))
            .expect("empty HAL directory creates");
        write_executable(&self.driver, "new-engine");
        write_file(&self.launchd, "new-launchd");
        write_executable(
            &self
                .remover
                .join("Contents/MacOS/mixrelay-driver-uninstaller"),
            "new-remover",
        );
        write_file(&self.remover.join("Contents/Info.plist"), "remover-info");
        write_file(&self.removal_launchd, "new-helper-plist");
        write_executable(&self.runtime_engine, "new-runtime");
        write_file(&self.runtime_launchd, "new-runtime-definition");
    }

    fn create_user_data(&self) {
        write_file(&self.user_data, "keep-this-data");
    }

    fn sources(&self) -> TransactionRequest {
        TransactionRequest::install(InstallSources::new(
            self.app.clone(),
            self.hal.clone(),
            (self.driver.clone(), self.launchd.clone()),
            (self.remover.clone(), self.removal_launchd.clone()),
            (self.runtime_engine.clone(), self.runtime_launchd.clone()),
        ))
    }

    fn adapter(&self) -> FilesystemAdapter {
        FilesystemAdapter::new(
            FilesystemLayout::new(self.root.clone()).expect("fixture layout validates"),
        )
    }

    fn destination(&self, target: OwnedTarget) -> PathBuf {
        self.root.join(target.path().trim_start_matches('/'))
    }

    fn create_old_targets(&self) {
        write_file(
            &self.destination(OwnedTarget::App).join("Contents/old.txt"),
            "old-app",
        );
        write_file(
            &self.destination(OwnedTarget::Hal).join("Contents/old.txt"),
            "old-hal",
        );
        write_file(&self.destination(OwnedTarget::Driver), "old-engine");
        write_file(&self.destination(OwnedTarget::Launchd), "old-launchd");
    }
}

fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture file has parent")).expect("parent creates");
    fs::write(path, contents).expect("fixture file writes");
}

fn write_executable(path: &Path, contents: &str) {
    write_file(path, contents);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("fixture executable mode sets");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("fixture file reads")
}

#[test]
fn preflight_and_stage_do_not_mutate_destinations_or_user_data() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = fixture.sources();

    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");

    assert_eq!(snapshot.presence(OwnedTarget::App), TargetPresence::Present);
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "old-engine"
    );
    assert_eq!(
        fs::symlink_metadata(staged.staging_root())
            .expect("staging metadata reads")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(
        staged
            .staging_root()
            .join("payload/app/Contents/Resources/empty")
            .is_dir()
    );
    adapter.cleanup(&staged).expect("stage cleanup succeeds");
    assert!(!adapter.layout().staging_root().exists());
}

#[test]
fn staging_normalizes_permissions_without_inheriting_privileged_or_writable_modes() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.app, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(&fixture.driver, fs::Permissions::from_mode(0o6755)).unwrap();
    fs::set_permissions(&fixture.launchd, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(
        fixture.app.join("Contents/Info.plist"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    for (path, mode) in [
        (staged.payload_path(OwnedTarget::App), 0o755),
        (staged.payload_path(OwnedTarget::Driver), 0o755),
        (staged.payload_path(OwnedTarget::Launchd), 0o644),
        (
            staged
                .payload_path(OwnedTarget::App)
                .join("Contents/Info.plist"),
            0o644,
        ),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }
    assert_eq!(
        fs::metadata(&fixture.driver).unwrap().permissions().mode() & 0o7777,
        0o6755
    );
    adapter.cleanup(&staged).unwrap();
}

#[test]
fn direct_copy_rejects_special_files_and_does_not_overwrite_an_existing_inode() {
    let fixture = Fixture::new();
    let fifo = fixture.root.join("fifo");
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let destination = fixture.root.join("copied");
    assert!(copy_tree(&fifo, &destination).is_err());
    assert!(super::payload::copy_regular_file(&fifo, &destination).is_err());
    assert!(!destination.exists());
    let alias = fixture.root.join("alias");
    symlink(&fixture.driver, &alias).unwrap();
    assert!(super::payload::copy_regular_file(&alias, &destination).is_err());
    assert!(!destination.exists());
    fs::write(&destination, b"keep-existing").unwrap();
    assert!(copy_tree(&fixture.driver, &destination).is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"keep-existing");
}

#[test]
fn receipts_are_bound_to_layout_request_and_snapshot() {
    let first = Fixture::new();
    let second = Fixture::new();
    let first_adapter = first.adapter();
    let second_adapter = second.adapter();
    let first_request = first.sources();
    let second_request = second.sources();
    let first_snapshot = first_adapter
        .preflight(&first_request)
        .expect("first preflight succeeds");
    let second_snapshot = second_adapter
        .preflight(&second_request)
        .expect("second preflight succeeds");
    let staged = first_adapter
        .stage(&first_request, &first_snapshot)
        .expect("first stage succeeds");

    let error = second_adapter
        .commit(&second_request, &second_snapshot, &staged)
        .expect_err("cross-layout receipt must reject before mutation");
    assert!(matches!(error, FilesystemError::ReceiptMismatch));
    assert!(!second.destination(OwnedTarget::App).exists());
    first_adapter
        .cleanup(&staged)
        .expect("first cleanup succeeds");
}

#[test]
fn install_commit_and_rollback_restore_exact_targets() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    let committed = adapter
        .commit(&request, &snapshot, &staged)
        .expect("commit succeeds");

    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "new-engine"
    );
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Launchd)),
        "new-launchd"
    );
    assert!(committed.backed_up_targets().contains(&OwnedTarget::App));

    adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .expect("rollback succeeds");
    assert_eq!(
        read(
            &fixture
                .destination(OwnedTarget::App)
                .join("Contents/old.txt")
        ),
        "old-app"
    );
    assert_eq!(
        read(
            &fixture
                .destination(OwnedTarget::Hal)
                .join("Contents/old.txt")
        ),
        "old-hal"
    );
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "old-engine"
    );
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Launchd)),
        "old-launchd"
    );
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    adapter.cleanup(&staged).expect("cleanup succeeds");
}

#[test]
fn successful_commit_cleanup_is_allowed_after_registration_boundary() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    adapter
        .commit(&request, &snapshot, &staged)
        .expect("commit succeeds");

    // The native caller invokes cleanup only after service registration succeeds.
    adapter
        .cleanup(&staged)
        .expect("committed staging cleanup succeeds");
    assert!(!adapter.layout().staging_root().exists());
}

#[test]
fn remove_commit_and_rollback_never_touch_user_data() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = TransactionRequest::remove();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    let committed = adapter
        .commit(&request, &snapshot, &staged)
        .expect("remove commit succeeds");

    for target in OwnedTarget::all() {
        assert!(!fixture.destination(target).exists());
    }
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .expect("remove rollback succeeds");
    assert!(fixture.destination(OwnedTarget::App).is_dir());
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "old-engine"
    );
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    adapter.cleanup(&staged).expect("cleanup succeeds");
}

#[test]
fn removal_succeeds_for_every_partial_target_set_and_on_repeat() {
    for mask in 0u16..(1 << OwnedTarget::all().len()) {
        let fixture = Fixture::new();
        for (index, target) in OwnedTarget::all().into_iter().enumerate() {
            if mask & (1 << index) != 0 {
                let path = fixture.destination(target);
                if target_kind(target) == TargetKind::Directory {
                    write_file(&path.join("owned"), "remove");
                } else {
                    write_file(&path, "remove");
                }
            }
        }
        let adapter = fixture.adapter();
        let request = TransactionRequest::remove();
        for _ in 0..2 {
            let snapshot = adapter.preflight(&request).unwrap();
            let staged = adapter.stage(&request, &snapshot).unwrap();
            adapter.commit(&request, &snapshot, &staged).unwrap();
            adapter.cleanup(&staged).unwrap();
            assert!(
                OwnedTarget::all()
                    .iter()
                    .all(|target| !fixture.destination(*target).exists())
            );
            assert_eq!(read(&fixture.user_data), "keep-this-data");
        }
    }
}

fn stale_stage(fixture: &Fixture) -> PathBuf {
    let path = fixture.adapter().layout().staging_root().to_path_buf();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(path.join("payload")).unwrap();
    fs::create_dir(path.join("backups")).unwrap();
    path
}

#[test]
fn restored_staging_is_automatically_reclaimed_for_install_and_repeat() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let path = stale_stage(&fixture);
    write_file(
        &path.join("payload/app/Contents/stale"),
        "unused new payload",
    );
    let adapter = fixture.adapter();
    adapter.cleanup_stale_staging().unwrap();
    adapter.cleanup_stale_staging().unwrap();
    adapter.preflight(&fixture.sources()).unwrap();
    assert!(!path.exists());
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "old-engine"
    );
    assert_eq!(read(&fixture.user_data), "keep-this-data");
}

#[test]
fn already_deleted_transaction_staging_is_successful_cleanup() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let request = TransactionRequest::Remove;
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    fs::remove_dir_all(staged.staging_root()).unwrap();
    adapter.cleanup(&staged).unwrap();
    adapter.cleanup(&staged).unwrap();
    assert!(!staged.staging_root().exists());
    assert_eq!(read(&fixture.user_data), "keep-this-data");
}

#[test]
fn abandoned_original_backups_are_cleaned_for_install_and_remove() {
    for install in [true, false] {
        let fixture = Fixture::new();
        let path = stale_stage(&fixture);
        write_file(&path.join("backups/driver"), "original driver");
        let request = if install {
            fixture.sources()
        } else {
            TransactionRequest::Remove
        };
        fixture.adapter().cleanup_stale_staging().unwrap();
        fixture.adapter().preflight(&request).unwrap();
        assert!(!path.exists());
        assert_eq!(read(&fixture.user_data), "keep-this-data");
    }
}

#[test]
fn stale_cleanup_removes_all_residue_without_following_external_links() {
    for case in ["unknown", "symlink", "hardlink", "writable"] {
        let fixture = Fixture::new();
        let path = stale_stage(&fixture);
        match case {
            "unknown" => write_file(&path.join("foreign"), "keep"),
            "symlink" => symlink(&fixture.user_data, path.join("payload/driver")).unwrap(),
            "hardlink" => fs::hard_link(&fixture.user_data, path.join("payload/driver")).unwrap(),
            _ => {
                write_file(&path.join("payload/driver"), "bad mode");
                fs::set_permissions(
                    path.join("payload/driver"),
                    fs::Permissions::from_mode(0o666),
                )
                .unwrap();
            }
        }
        fixture.adapter().cleanup_stale_staging().unwrap();
        assert!(!path.exists());
        assert_eq!(read(&fixture.user_data), "keep-this-data");
    }
}

#[test]
fn target_symlink_is_rejected_before_any_mutation() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let hal = fixture.destination(OwnedTarget::Hal);
    let old_hal = fixture.root.join("old-hal-outside-target");
    fs::rename(&hal, &old_hal).expect("old HAL moves");
    symlink(&old_hal, &hal).expect("HAL symlink creates");
    let adapter = fixture.adapter();

    let error = adapter
        .preflight(&fixture.sources())
        .expect_err("symlink target must reject preflight");
    assert!(matches!(error, FilesystemError::UnsafePath { .. }));
    assert!(hal.is_symlink());
    assert_eq!(read(&fixture.user_data), "keep-this-data");
}

#[test]
fn partial_commit_failure_rolls_back_real_fixture_files() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    fs::remove_dir_all(staged.staging_root().join("payload/hal"))
        .expect("HAL payload removal creates a commit failure");

    let error = adapter
        .commit(&request, &snapshot, &staged)
        .expect_err("missing payload must fail after earlier renames");
    assert!(matches!(error, FilesystemError::Io { .. }));
    adapter
        .rollback(&request, &snapshot, &staged, None)
        .expect("partial commit rollback succeeds");
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "old-engine"
    );
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Launchd)),
        "old-launchd"
    );
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    adapter.cleanup(&staged).expect("cleanup succeeds");
}

#[test]
fn created_parent_replacement_is_retained_until_identity_is_restored() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    let committed = adapter
        .commit(&request, &snapshot, &staged)
        .expect("commit succeeds");

    // Every launchd definition shares this created parent; move all of them so
    // only the parent's identity differs.
    let definitions = [
        OwnedTarget::Launchd,
        OwnedTarget::RemovalLaunchd,
        OwnedTarget::RuntimeLaunchd,
    ]
    .map(|target| fixture.destination(target));
    let launchd_parent = definitions[0]
        .parent()
        .expect("launchd has parent")
        .to_path_buf();
    let moved_parent = fixture.root.join("launchd-parent-outside-target");
    let outside = |index: usize| fixture.root.join(format!("launchd-outside-parent-{index}"));
    for (index, definition) in definitions.iter().enumerate() {
        fs::rename(definition, outside(index)).expect("definition moves");
    }
    fs::rename(&launchd_parent, &moved_parent).expect("created parent moves");
    fs::create_dir(&launchd_parent).expect("replacement parent creates");
    for (index, definition) in definitions.iter().enumerate() {
        fs::rename(outside(index), definition).expect("definition returns to replacement parent");
    }

    let failure = adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .expect_err("replacement parent identity must retain evidence");
    assert!(matches!(
        failure.error(),
        FilesystemError::CreatedDirectoryChanged { .. }
    ));
    assert!(failure.diagnostics().staging_root().is_dir());
    assert!(definitions.iter().all(|definition| !definition.exists()));

    fs::remove_dir(&launchd_parent).expect("replacement parent removes");
    fs::rename(&moved_parent, &launchd_parent).expect("original parent identity restores");
    adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .expect("rollback retry removes only original created parents");
    adapter
        .cleanup(&staged)
        .expect("cleanup succeeds after retry");
}

#[test]
fn staging_replacement_is_rejected_without_recursive_cleanup() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    let staging_root = staged.staging_root().to_path_buf();
    let moved_staging = fixture.root.join("staging-outside-target");
    fs::rename(&staging_root, &moved_staging).expect("staging moves");
    fs::create_dir(&staging_root).expect("replacement staging creates");

    let error = adapter
        .commit(&request, &snapshot, &staged)
        .expect_err("staging identity replacement must reject commit");
    assert!(matches!(
        error,
        FilesystemError::StagingIdentityChanged { .. }
    ));
    assert!(moved_staging.is_dir());
    assert!(staging_root.is_dir());
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    assert!(matches!(
        adapter.cleanup(&staged),
        Err(FilesystemError::StagingIdentityChanged { .. })
    ));
    fs::remove_dir(&staging_root).expect("remove empty replacement fixture directory");
    fs::rename(&moved_staging, &staging_root).expect("restore original staging inode");
    adapter
        .cleanup(&staged)
        .expect("same receipt can retry cleanup");
    assert!(!staging_root.exists());
}

#[test]
fn rollback_failure_cleans_backups_without_changing_foreign_targets() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).expect("preflight succeeds");
    let staged = adapter.stage(&request, &snapshot).expect("stage succeeds");
    let committed = adapter
        .commit(&request, &snapshot, &staged)
        .expect("commit succeeds");
    let engine = fixture.destination(OwnedTarget::Driver);
    let moved_engine = fixture.root.join("engine-outside-target");
    fs::rename(&engine, &moved_engine).expect("new engine moves");
    symlink(&moved_engine, &engine).expect("engine symlink creates");

    let failure = adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .expect_err("rollback must retain evidence when target is replaced");
    assert!(failure.diagnostics().staging_root().is_dir());
    assert!(staged.staging_root().is_dir());
    assert!(!fixture.destination(OwnedTarget::Launchd).exists());
    assert!(!fixture.destination(OwnedTarget::Hal).exists());
    assert_eq!(read(&fixture.user_data), "keep-this-data");
    adapter
        .cleanup(&staged)
        .expect("failed rollback still cleans staging");
    assert!(!staged.staging_root().exists());
    assert!(engine.is_symlink());
    assert_eq!(read(&moved_engine), "new-engine");
    assert_eq!(read(&fixture.user_data), "keep-this-data");
}

#[test]
fn install_request_shape_remains_explicit() {
    let fixture = Fixture::new();
    assert_eq!(fixture.sources().kind(), OperationKind::Install);
    assert_eq!(TransactionRequest::remove().kind(), OperationKind::Remove);
}

#[test]
fn removal_app_and_helper_are_added_and_restored_as_one_payload_set() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let request = fixture.sources();
    let adapter = fixture.adapter();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    let committed = adapter.commit(&request, &snapshot, &staged).unwrap();
    assert_eq!(
        read(
            &fixture
                .destination(OwnedTarget::Uninstaller)
                .join("Contents/MacOS/mixrelay-driver-uninstaller")
        ),
        "new-remover"
    );
    assert_eq!(
        read(&fixture.destination(OwnedTarget::RemovalLaunchd)),
        "new-helper-plist"
    );
    adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .unwrap();
    assert!(!fixture.destination(OwnedTarget::Uninstaller).exists());
    assert!(!fixture.destination(OwnedTarget::RemovalLaunchd).exists());
    assert_eq!(
        read(&fixture.destination(OwnedTarget::Driver)),
        "old-engine"
    );
    adapter.cleanup(&staged).unwrap();
}

#[test]
fn driver_runtime_is_committed_restored_and_removed_with_its_payload() {
    let fixture = Fixture::new();
    write_executable(
        &fixture.destination(OwnedTarget::RuntimeEngine),
        "old-runtime",
    );
    write_file(
        &fixture.destination(OwnedTarget::RuntimeLaunchd),
        "old-runtime-definition",
    );
    let request = fixture.sources();
    let adapter = fixture.adapter();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    let committed = adapter.commit(&request, &snapshot, &staged).unwrap();
    assert_eq!(
        read(&fixture.destination(OwnedTarget::RuntimeEngine)),
        "new-runtime"
    );
    adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .unwrap();
    assert_eq!(
        read(&fixture.destination(OwnedTarget::RuntimeEngine)),
        "old-runtime"
    );
    assert_eq!(
        read(&fixture.destination(OwnedTarget::RuntimeLaunchd)),
        "old-runtime-definition"
    );
    adapter.cleanup(&staged).unwrap();
    let request = TransactionRequest::remove();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    adapter.commit(&request, &snapshot, &staged).unwrap();
    adapter.cleanup(&staged).unwrap();
    assert!(!fixture.destination(OwnedTarget::RuntimeEngine).exists());
    assert!(!fixture.destination(OwnedTarget::RuntimeLaunchd).exists());
    assert_eq!(read(&fixture.user_data), "keep-this-data");
}

#[test]
fn first_install_creates_product_folder_and_rollback_removes_it() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let folder = adapter.product_directory_path();
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    assert!(!folder.exists());
    let committed = adapter.commit(&request, &snapshot, &staged).unwrap();
    assert_eq!(
        fs::symlink_metadata(&folder).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    assert!(fixture.destination(OwnedTarget::App).is_dir());
    assert!(fixture.destination(OwnedTarget::Uninstaller).is_dir());
    adapter
        .rollback(&request, &snapshot, &staged, Some(&committed))
        .unwrap();
    assert!(!folder.exists());
    adapter.cleanup(&staged).unwrap();
}

#[test]
fn unsafe_product_folder_is_refused_without_mutation() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let folder = adapter.product_directory_path();
    fs::create_dir_all(&folder).unwrap();
    write_file(&folder.join("user-file"), "keep");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o775)).unwrap();
    assert!(matches!(
        adapter.preflight(&fixture.sources()),
        Err(FilesystemError::UnsafePath { .. })
    ));
    assert_eq!(read(&folder.join("user-file")), "keep");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    let outside = fixture.root.join("outside-product");
    fs::rename(&folder, &outside).unwrap();
    symlink(&outside, &folder).unwrap();
    assert!(matches!(
        adapter.preflight(&TransactionRequest::remove()),
        Err(FilesystemError::UnsafePath { .. })
    ));
    assert_eq!(read(&outside.join("user-file")), "keep");
    assert!(!adapter.layout().staging_root().exists());
}

#[test]
fn successful_removal_cleans_only_the_validated_empty_product_folder() {
    for user_file in [false, true] {
        let fixture = Fixture::new();
        let adapter = fixture.adapter();
        let install = fixture.sources();
        let snapshot = adapter.preflight(&install).unwrap();
        let staged = adapter.stage(&install, &snapshot).unwrap();
        adapter.commit(&install, &snapshot, &staged).unwrap();
        adapter.cleanup(&staged).unwrap();
        let folder = adapter.product_directory_path();
        if user_file {
            write_file(&folder.join("user-file"), "keep");
        }
        let remove = TransactionRequest::remove();
        let snapshot = adapter.preflight(&remove).unwrap();
        let staged = adapter.stage(&remove, &snapshot).unwrap();
        let committed = adapter.commit(&remove, &snapshot, &staged).unwrap();
        // Registration may still fail: retain the original folder for rollback.
        assert!(folder.is_dir());
        adapter
            .rollback(&remove, &snapshot, &staged, Some(&committed))
            .unwrap();
        assert!(fixture.destination(OwnedTarget::App).is_dir());
        assert!(fixture.destination(OwnedTarget::Uninstaller).is_dir());
        adapter.cleanup(&staged).unwrap();
        let snapshot = adapter.preflight(&remove).unwrap();
        let staged = adapter.stage(&remove, &snapshot).unwrap();
        adapter.commit(&remove, &snapshot, &staged).unwrap();
        adapter.cleanup(&staged).unwrap();
        adapter
            .remove_empty_product_directory(snapshot.product_directory)
            .unwrap();
        assert_eq!(folder.exists(), user_file);
        assert!(
            OwnedTarget::all()
                .into_iter()
                .all(|target| !fixture.destination(target).exists())
        );
        if user_file {
            assert_eq!(read(&folder.join("user-file")), "keep");
        }
    }
}

#[test]
fn removal_never_cleans_a_replacement_product_directory() {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let request = TransactionRequest::remove();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    adapter.commit(&request, &snapshot, &staged).unwrap();
    let folder = adapter.product_directory_path();
    let retained = fixture.root.join("validated-folder");
    fs::rename(&folder, &retained).unwrap();
    fs::create_dir(&folder).unwrap();
    assert!(matches!(
        adapter.cleanup(&staged),
        Err(FilesystemError::CreatedDirectoryChanged { .. })
    ));
    assert!(folder.is_dir());
    assert!(retained.is_dir());
    fs::remove_dir(&folder).unwrap();
    fs::rename(&retained, &folder).unwrap();
    adapter.cleanup(&staged).unwrap();
    assert!(!folder.exists());
}
