use crate::{ClapScanner, scanner::scan_optional_default_dir_into};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use truce_rack_core::error::Error;
use truce_rack_core::info::PluginInfo;
use truce_rack_core::scanner::PluginScanner;

static NEXT_FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

struct ScannerFixture {
    root: PathBuf,
}

impl ScannerFixture {
    fn new() -> Self {
        let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "truce-rack-clap-scanner-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create unique CLAP scanner fixture directory");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn create_dir(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        fs::create_dir(&path).expect("create CLAP scanner fixture subdirectory");
        path
    }
}

impl Drop for ScannerFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn missing_default_directory_is_optional() {
    let fixture = ScannerFixture::new();
    let missing = fixture.path("not-installed");
    let mut plugins = Vec::<PluginInfo>::new();

    scan_optional_default_dir_into(&missing, &mut plugins)
        .expect("an absent conventional directory is optional");

    assert!(plugins.is_empty());
}

#[test]
fn explicit_missing_path_is_an_informative_error() {
    let fixture = ScannerFixture::new();
    let missing = fixture.path("not-installed");
    let error = ClapScanner::new()
        .scan_path(&missing)
        .expect_err("an explicit path must not turn absence into an empty scan");

    assert!(error.to_string().contains(&missing.display().to_string()));
}

#[test]
fn explicit_empty_directory_is_a_successful_empty_scan() {
    let fixture = ScannerFixture::new();
    let empty = fixture.create_dir("empty");

    let plugins = ClapScanner::new()
        .scan_path(&empty)
        .expect("an existing empty directory is a valid scan target");

    assert!(plugins.is_empty());
}

#[test]
fn exact_regular_clap_path_reports_bundle_load_failure() {
    let fixture = ScannerFixture::new();
    let bundle = fixture.path("corrupt.clap");
    fs::write(&bundle, b"not a shared library").expect("write corrupt CLAP bundle fixture");

    let error = ClapScanner::new()
        .scan_path(&bundle)
        .expect_err("a regular .clap file is scanned as a bundle and must fail to load");

    assert!(matches!(
        &error,
        Error::LoadFailed { path, .. } if path == &bundle
    ));
    assert!(error.to_string().contains(&bundle.display().to_string()));
}

#[test]
fn directory_scan_reports_corrupt_clap_candidate() {
    let fixture = ScannerFixture::new();
    let directory = fixture.create_dir("plugins");
    let corrupt = directory.join("corrupt.clap");
    fs::write(&corrupt, b"not a shared library").expect("write corrupt CLAP candidate");
    fs::write(directory.join("notes.txt"), b"not a plugin")
        .expect("write non-plugin directory entry");

    let error = ClapScanner::new()
        .scan_path(&directory)
        .expect_err("a broken .clap candidate must fail instead of producing a fake empty scan");

    assert!(matches!(
        &error,
        Error::LoadFailed { path, .. } if path == &corrupt
    ));
    assert!(error.to_string().contains(&corrupt.display().to_string()));
}

#[cfg(unix)]
#[test]
fn unreadable_directory_errors_when_the_os_enforces_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = ScannerFixture::new();
    let unreadable = fixture.create_dir("unreadable");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
        .expect("remove directory read permissions");
    let permission_denied = match fs::read_dir(&unreadable) {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => true,
        Err(error) => panic!("permission probe failed unexpectedly: {error}"),
        Ok(entries) => {
            drop(entries);
            false
        }
    };

    let scan_result = ClapScanner::new().scan_path(&unreadable);
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700))
        .expect("restore fixture directory permissions");

    if permission_denied {
        let error = scan_result.expect_err("unreadable explicit directories must fail");
        assert!(
            error
                .to_string()
                .contains(&unreadable.display().to_string())
        );
        assert!(error.to_string().contains("read explicit CLAP directory"));
    } else {
        assert!(matches!(scan_result, Ok(plugins) if plugins.is_empty()));
    }
}
