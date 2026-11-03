//! Empty-only removal under namespace substitution and restoration conflicts.

use super::*;
use std::os::unix::fs::{MetadataExt, symlink};

#[test]
fn production_product_folder_refuses_non_root_and_writable_owners() {
    assert!(product_directory_policy(0, 0o755, 0));
    assert!(!product_directory_policy(501, 0o755, 0));
    assert!(!product_directory_policy(0, 0o775, 0));
    assert!(!product_directory_policy(0, 0o757, 0));
}

fn fixture() -> (mixrelay_test_support::FixtureDir, FilesystemAdapter) {
    let root = mixrelay_test_support::FixtureDir::new("product-remove").unwrap();
    let adapter = FilesystemAdapter::new(super::super::FilesystemLayout::new(root.path()).unwrap());
    fs::create_dir_all(adapter.layout.staging_root().parent().unwrap()).unwrap();
    fs::create_dir_all(adapter.product_directory_path()).unwrap();
    (root, adapter)
}

#[test]
fn removal_race_preserves_substituted_empty_directory_and_symlink() {
    for replace_with_symlink in [false, true] {
        let (root, adapter) = fixture();
        let folder = adapter.product_directory_path();
        let expected = adapter.observe_product_directory().unwrap();
        let outside = root.join("unrelated");
        fs::create_dir(&outside).unwrap();
        let before = fs::metadata(&outside).unwrap();
        let retained = root.join("validated");
        assert!(
            adapter
                .remove_empty_product_directory_with(expected, || {
                    fs::rename(&folder, &retained).unwrap();
                    if replace_with_symlink {
                        symlink(&outside, &folder).unwrap();
                    } else {
                        fs::rename(&outside, &folder).unwrap();
                    }
                })
                .is_err()
        );
        let unrelated = if replace_with_symlink {
            &outside
        } else {
            &folder
        };
        assert_eq!(fs::metadata(unrelated).unwrap().ino(), before.ino());
        assert_eq!(
            fs::metadata(retained).unwrap().ino(),
            expected.unwrap().inode
        );
        assert_eq!(
            fs::symlink_metadata(&folder)
                .unwrap()
                .file_type()
                .is_symlink(),
            replace_with_symlink
        );
        assert!(
            !adapter
                .layout
                .staging_root()
                .parent()
                .unwrap()
                .join(".product-folder-holding")
                .exists()
        );
    }
}

#[test]
fn removal_restores_a_validated_folder_that_became_nonempty() {
    let (_root, adapter) = fixture();
    let folder = adapter.product_directory_path();
    let expected = adapter.observe_product_directory().unwrap();
    adapter
        .remove_empty_product_directory_with(expected, || {
            fs::write(folder.join("keep"), b"user content").unwrap();
        })
        .unwrap();
    assert_eq!(
        fs::metadata(&folder).unwrap().ino(),
        expected.unwrap().inode
    );
    assert_eq!(fs::read(folder.join("keep")).unwrap(), b"user content");
}

#[test]
fn restoration_conflict_retains_captured_content_outside_transaction_staging() {
    let (_root, adapter) = fixture();
    let folder = adapter.product_directory_path();
    let expected = adapter.observe_product_directory().unwrap();
    let lifecycle = adapter.layout.staging_root().parent().unwrap();
    let holding_path = lifecycle.join(".product-folder-holding");
    fs::create_dir(&holding_path).unwrap();
    let applications = open_directory(folder.parent().unwrap()).unwrap();
    let holding = open_directory(&holding_path).unwrap();
    let captured = holding_path.join("MixRelay");
    fs::write(folder.join("keep"), b"captured content").unwrap();
    assert!(
        capture_empty_product(&applications, &holding, expected, &folder, || {
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("keep"), b"competing content").unwrap();
        })
        .is_err()
    );
    assert_eq!(fs::read(folder.join("keep")).unwrap(), b"competing content");
    assert_eq!(
        fs::read(captured.join("keep")).unwrap(),
        b"captured content"
    );
    assert_eq!(
        fs::metadata(&captured).unwrap().ino(),
        expected.unwrap().inode
    );
    assert_ne!(
        fs::metadata(&folder).unwrap().ino(),
        expected.unwrap().inode
    );
}
