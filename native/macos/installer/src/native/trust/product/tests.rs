//! Publication races use fixture parents and the current unprivileged identity.

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

use rustix::process::{getegid, geteuid};

use crate::directory::open_directory;

#[test]
fn publication_race_preserves_competing_directory_and_symlink() {
    for replace_with_symlink in [false, true] {
        let root = mixrelay_test_support::FixtureDir::new("product-publish").unwrap();
        let applications_path = root.join("Applications");
        let lifecycle_path = root.join("lifecycle");
        fs::create_dir(&applications_path).unwrap();
        fs::create_dir(&lifecycle_path).unwrap();
        let applications = open_directory(&applications_path).unwrap();
        let lifecycle = open_directory(&lifecycle_path).unwrap();
        let outside = root.join("unrelated");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o750)).unwrap();
        fs::write(outside.join("keep"), b"unrelated content").unwrap();
        let before = fs::metadata(&outside).unwrap();
        let public = applications_path.join("MixRelay");
        let result = super::create_product_directory_at(
            &applications,
            &lifecycle,
            (geteuid(), getegid()),
            || {
                // Also models replacement after the old public mkdirat.
                if public.exists() {
                    fs::remove_dir(&public).unwrap();
                }
                if replace_with_symlink {
                    symlink(&outside, &public).unwrap();
                } else {
                    fs::rename(&outside, &public).unwrap();
                }
            },
        );
        let retained = if replace_with_symlink {
            &outside
        } else {
            &public
        };
        let after = fs::metadata(retained).unwrap();
        assert_eq!(
            (
                after.dev(),
                after.ino(),
                after.uid(),
                after.gid(),
                after.mode()
            ),
            (
                before.dev(),
                before.ino(),
                before.uid(),
                before.gid(),
                before.mode()
            )
        );
        assert_eq!(
            fs::read(retained.join("keep")).unwrap(),
            b"unrelated content"
        );
        if replace_with_symlink {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().metadata().unwrap().ino(), before.ino());
        }
        assert!(!lifecycle_path.join(".product-folder-create").exists());
    }
}

#[test]
fn publication_configures_only_the_private_inode_and_retains_its_descriptor() {
    let root = mixrelay_test_support::FixtureDir::new("product-publish-owned").unwrap();
    let applications = open_directory(&root).unwrap();
    let lifecycle_path = root.join("lifecycle");
    fs::create_dir(&lifecycle_path).unwrap();
    let lifecycle = open_directory(&lifecycle_path).unwrap();
    let directory = super::create_product_directory_at(
        &applications,
        &lifecycle,
        (geteuid(), getegid()),
        || {
            assert!(!root.join("MixRelay").exists());
            let staged =
                fs::metadata(lifecycle_path.join(".product-folder-create/MixRelay")).unwrap();
            assert_eq!(staged.mode() & 0o7777, 0o755);
        },
    )
    .unwrap();
    let before = directory.metadata().unwrap();
    fs::rename(root.join("MixRelay"), root.join("retained")).unwrap();
    fs::create_dir(root.join("MixRelay")).unwrap();
    assert_eq!(directory.metadata().unwrap().ino(), before.ino());
    assert_eq!(
        fs::metadata(root.join("retained")).unwrap().ino(),
        before.ino()
    );
}
