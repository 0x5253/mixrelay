//! Behavioral fixtures for the adjacent native owner.

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn factory_subcategories_distinguish_instruments_from_insert_effects() {
    assert_eq!(
        class_category("Instrument|Sampler"),
        PluginCategory::Instrument
    );
    assert_eq!(
        class_category("Fx|Instrument|Sampler"),
        PluginCategory::Instrument
    );
    assert_eq!(class_category("Fx|Dynamics"), PluginCategory::Effect);
    assert_eq!(class_category("Fx|Instrumental"), PluginCategory::Effect);
    assert_eq!(class_category(""), PluginCategory::Effect);
}

struct ScannerFixture {
    root: PathBuf,
}

impl ScannerFixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock follows Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "truce-rack-vst3-scan-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create isolated scanner fixture");
        Self { root }
    }

    fn child(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for ScannerFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn vst3_scanner_distinguishes_optional_default_dirs_from_explicit_paths() {
    let fixture = ScannerFixture::new();
    let optional_missing = fixture.child("optional-default");
    let mut plugins = Vec::new();
    scan_optional_default_dir(&optional_missing, &mut plugins)
        .expect("a missing conventional directory is optional");
    assert!(plugins.is_empty());

    let explicit_missing = fixture.child("explicit-missing");
    let error = Vst3Scanner::new()
        .scan_path(&explicit_missing)
        .expect_err("an explicit missing directory is an error");
    assert!(
        error
            .to_string()
            .contains(&explicit_missing.display().to_string())
    );

    let not_directory = fixture.child("not-a-directory");
    std::fs::write(&not_directory, b"file").expect("create non-directory scan target");
    let error = Vst3Scanner::new()
        .scan_path(&not_directory)
        .expect_err("an unreadable explicit scan target is an error");
    assert!(
        error
            .to_string()
            .contains(&not_directory.display().to_string())
    );

    let empty = fixture.child("empty");
    std::fs::create_dir(&empty).expect("create empty directory");
    assert!(
        Vst3Scanner::new()
            .scan_path(&empty)
            .expect("an existing empty directory scans successfully")
            .is_empty()
    );
}

#[test]
fn vst3_scanner_supports_exact_bundle_paths_and_propagates_corruption() {
    let fixture = ScannerFixture::new();
    let corrupt_bundle = fixture.child("corrupt.vst3");
    std::fs::create_dir(&corrupt_bundle).expect("create empty corrupt bundle");
    let exact_error = Vst3Scanner::new()
        .scan_path(&corrupt_bundle)
        .expect_err("an exact bundle path is opened as a bundle and errors");
    assert!(
        exact_error
            .to_string()
            .contains(&corrupt_bundle.display().to_string())
    );

    let directory = fixture.child("plugins");
    std::fs::create_dir(&directory).expect("create plugin directory");
    let nested_corrupt = directory.join("broken.vst3");
    std::fs::create_dir(&nested_corrupt).expect("create corrupt candidate");
    let directory_error = Vst3Scanner::new()
        .scan_path(&directory)
        .expect_err("a corrupt candidate fails the directory scan");
    assert!(
        directory_error
            .to_string()
            .contains(&nested_corrupt.display().to_string())
    );
}
