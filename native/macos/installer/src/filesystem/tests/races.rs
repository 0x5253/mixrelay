//! Deterministic substitutions after descriptor validation and before mutation.

use super::*;
use std::os::unix::fs::MetadataExt;

fn create_unrelated_apps(path: &Path) -> u64 {
    for target in [OwnedTarget::App, OwnedTarget::Uninstaller] {
        let name = Path::new(target.path()).file_name().unwrap();
        write_file(&path.join(name).join("keep"), "unrelated content");
    }
    fs::metadata(path).unwrap().ino()
}

#[test]
fn app_renames_and_rollback_stay_in_validated_folder_after_substitution() {
    for replace_with_symlink in [false, true] {
        check_app_folder_substitution(replace_with_symlink);
    }
}

fn check_app_folder_substitution(replace_with_symlink: bool) {
    let fixture = Fixture::new();
    fixture.create_old_targets();
    let adapter = fixture.adapter();
    let folder = adapter.product_directory_path();
    write_file(
        &fixture.destination(OwnedTarget::Uninstaller).join("old"),
        "old-remover",
    );
    let request = fixture.sources();
    let snapshot = adapter.preflight(&request).unwrap();
    let staged = adapter.stage(&request, &snapshot).unwrap();
    let mut progress = MutationProgress::new();
    let product = adapter
        .open_product_directory(&snapshot, &progress)
        .unwrap()
        .unwrap();
    let retained = fixture.root.join("validated-folder");
    let outside = fixture.root.join("unrelated");
    let outside_inode = create_unrelated_apps(&outside);
    fs::rename(&folder, &retained).unwrap();
    if replace_with_symlink {
        symlink(&outside, &folder).unwrap();
    } else {
        fs::rename(&outside, &folder).unwrap();
    }
    let context = CommitContext {
        request: &request,
        snapshot: &snapshot,
        staged: &staged,
    };
    for target in [OwnedTarget::App, OwnedTarget::Uninstaller] {
        adapter
            .commit_target_at(&context, target, Some(&product), &mut progress)
            .unwrap();
    }
    assert_eq!(
        read(
            &staged
                .backup_path(OwnedTarget::App)
                .join("Contents/old.txt")
        ),
        "old-app"
    );
    assert_eq!(
        read(&staged.backup_path(OwnedTarget::Uninstaller).join("old")),
        "old-remover"
    );
    assert_eq!(
        read(&retained.join("MixRelay.app/Contents/MacOS/mixrelay-desktop")),
        "new-app"
    );
    // The final pathname snapshot rejects the substitution after restoring via the FD.
    assert!(
        adapter
            .restore_progress_at(&snapshot, &staged, &mut progress, Some(&product))
            .is_err()
    );
    assert!(progress.backups().is_empty());
    assert!(progress.installed().is_empty());
    assert_eq!(
        read(&retained.join("MixRelay.app/Contents/old.txt")),
        "old-app"
    );
    assert_eq!(
        read(&retained.join("Uninstall MixRelay.app/old")),
        "old-remover"
    );
    adapter.cleanup(&staged).unwrap();
    let unrelated = if replace_with_symlink {
        &outside
    } else {
        &folder
    };
    assert_eq!(fs::metadata(unrelated).unwrap().ino(), outside_inode);
    for name in ["MixRelay.app", "Uninstall MixRelay.app"] {
        assert_eq!(
            read(&unrelated.join(name).join("keep")),
            "unrelated content"
        );
    }
}
