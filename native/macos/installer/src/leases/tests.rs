use super::*;
use std::{
    os::unix::fs::{MetadataExt, symlink},
    path::PathBuf,
};

struct Fixture {
    _root: mixrelay_test_support::FixtureDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let owner = mixrelay_test_support::FixtureDir::new("macos-leases").unwrap();
        let root = owner.path().to_path_buf();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _root: owner,
            path: root,
        }
    }

    fn directory(&self) -> LifecycleDirectory {
        LifecycleDirectory::open(self.path.clone()).unwrap()
    }
}

#[test]
fn each_busy_owner_is_reported_and_partial_acquisition_is_released() {
    for kind in [LeaseKind::App, LeaseKind::Engine, LeaseKind::Installer] {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        let owner = directory.acquire_lock(kind, true).unwrap();
        assert!(
            matches!(directory.acquire(), Err(LeaseError::Busy { kind: found }) if found == kind)
        );
        if kind != LeaseKind::App {
            drop(directory.acquire_lock(LeaseKind::App, true).unwrap());
        }
        if kind == LeaseKind::Installer {
            drop(directory.acquire_lock(LeaseKind::Engine, true).unwrap());
        }
        drop(owner);
        drop(directory.acquire().unwrap());
    }
}

#[test]
fn guard_excludes_process_owners_and_other_installers_until_drop() {
    let fixture = Fixture::new();
    let directory = fixture.directory();
    let guard = directory.acquire().unwrap();
    for kind in [LeaseKind::App, LeaseKind::Engine, LeaseKind::Installer] {
        assert!(matches!(
            directory.acquire_lock(kind, true),
            Err(LeaseError::Busy { .. })
        ));
    }
    assert!(matches!(
        directory.acquire(),
        Err(LeaseError::Busy {
            kind: LeaseKind::Installer
        })
    ));
    drop(guard);
    for kind in [LeaseKind::App, LeaseKind::Engine, LeaseKind::Installer] {
        drop(directory.acquire_lock(kind, true).unwrap());
    }
}

#[test]
fn owned_lock_drop_releases_locks_even_when_inherited_descriptors_remain_open() {
    let fixture = Fixture::new();
    let directory = fixture.directory();
    for kind in [LeaseKind::App, LeaseKind::Engine, LeaseKind::Installer] {
        let owner = directory.acquire_lock(kind, true).unwrap();
        // A fork shares this open-file description until exec closes CLOEXEC FDs.
        let inherited = owner.0.try_clone().unwrap();
        drop(owner);
        drop(directory.acquire_lock(kind, true).unwrap());
        drop(inherited);
    }
}

#[test]
fn acquisition_never_replaces_or_truncates_existing_inodes() {
    let fixture = Fixture::new();
    let directory = fixture.directory();
    let path = directory.lease_path(LeaseKind::App);
    fs::write(&path, b"retained owner metadata").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&path).unwrap();
    drop(directory.acquire().unwrap());
    let after = fs::metadata(&path).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(fs::read(&path).unwrap(), b"retained owner metadata");
}

#[test]
fn rejects_symlinks_nonregular_files_and_unsafe_modes() {
    let fixture = Fixture::new();
    let directory = fixture.directory();
    let lease = directory.lease_path(LeaseKind::App);
    symlink("missing", &lease).unwrap();
    assert!(matches!(
        directory.acquire(),
        Err(LeaseError::UnsafePath { .. })
    ));
    fs::remove_file(&lease).unwrap();
    fs::create_dir(&lease).unwrap();
    assert!(matches!(
        directory.acquire(),
        Err(LeaseError::UnexpectedKind { .. })
    ));
    fs::remove_dir(&lease).unwrap();
    fs::write(&lease, b"").unwrap();
    for mode in [0o666, 0o755] {
        fs::set_permissions(&lease, fs::Permissions::from_mode(mode)).unwrap();
        assert!(matches!(
            directory.acquire(),
            Err(LeaseError::UnsafeMode { .. })
        ));
    }
}

#[test]
fn directory_is_revalidated_before_acquisition() {
    let fixture = Fixture::new();
    let directory = fixture.directory();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        directory.acquire(),
        Err(LeaseError::UnsafeMode { .. })
    ));
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = fixture.path.join("alias");
    symlink(&fixture.path, &alias).unwrap();
    assert!(matches!(
        LifecycleDirectory::open(alias),
        Err(LeaseError::UnsafePath { .. })
    ));
    assert!(LifecycleDirectory::open("relative").is_err());
    assert!(LifecycleDirectory::open(fixture.path.join("..")).is_err());
}

struct ChildOwner(std::process::Child);

#[test]
fn replaced_directory_cannot_redirect_a_previously_opened_owner() {
    let fixture = Fixture::new();
    let path = fixture.path.join("lifecycle");
    fs::create_dir(&path).unwrap();
    let directory = LifecycleDirectory::open(&path).unwrap();
    let retained = fixture.path.join("retained");
    fs::rename(&path, &retained).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        directory.acquire(),
        Err(LeaseError::UnsafePath { .. })
    ));
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    // Individual opens are anchored to the retained descriptor, not the new path.
    drop(directory.acquire_lock(LeaseKind::App, true).unwrap());
    assert!(retained.join(APP_LEASE_NAME).is_file());
    assert!(!path.join(APP_LEASE_NAME).exists());
}

#[test]
fn hard_linked_lease_cannot_alias_an_unrelated_owner() {
    let fixture = Fixture::new();
    let other = fixture.path.join("unrelated");
    fs::write(&other, b"preserved").unwrap();
    fs::hard_link(&other, fixture.path.join(APP_LEASE_NAME)).unwrap();
    assert!(matches!(
        fixture.directory().acquire(),
        Err(LeaseError::UnsafePath { .. })
    ));
    assert_eq!(fs::read(&other).unwrap(), b"preserved");
}

impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn independent_process_owner_excludes_installer_and_death_releases_lock() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_DIRECTORY: &str = "MIXRELAY_LEASE_TEST_DIRECTORY";
    if let Some(path) = std::env::var_os(CHILD_DIRECTORY) {
        let directory = LifecycleDirectory::open(PathBuf::from(path)).unwrap();
        let _owner = directory.acquire_lock(LeaseKind::Engine, true).unwrap();
        fs::write(directory.path().join("ready"), b"locked").unwrap();
        // The parent owns this pipe. EOF also releases the process if it dies.
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut [0_u8]);
        return;
    }
    let fixture = Fixture::new();
    let directory = fixture.directory();
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "leases::tests::independent_process_owner_excludes_installer_and_death_releases_lock",
            ])
            .env(CHILD_DIRECTORY, &fixture.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.path.join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before locking"
        );
        assert!(Instant::now() < deadline, "child did not acquire lock");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        directory.acquire(),
        Err(LeaseError::Busy {
            kind: LeaseKind::Engine
        })
    ));
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    drop(directory.acquire().unwrap());
}
