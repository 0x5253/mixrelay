use std::{
    fs::{self, File},
    io,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

use mixrelay_test_support::FixtureDir;

use super::*;

fn lease_fixture(label: &str) -> (FixtureDir, [File; 2]) {
    let root = FixtureDir::new(label).expect("lease fixture");
    for name in USER_LEASE_NAMES.into_iter().chain([INSTALLER_LEASE_NAME]) {
        let path = root.join(name);
        fs::write(&path, name).expect("fixture lease");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("lease mode");
    }
    let open = |name| {
        File::options()
            .read(true)
            .write(true)
            .open(root.join(name))
            .expect("fixture descriptor")
    };
    let leases = [open("app.owner"), open("engine.owner")];
    (root, leases)
}

#[test]
fn cold_reinstall_rebinds_only_user_owners_under_the_existing_guard() {
    let (root, leases) = lease_fixture("lease-rebind");
    let directory = LifecycleDirectory::open(root.path()).expect("directory");
    let guard = directory.acquire().expect("idle owners");
    let previous = inspect_user_lease_owners(&leases).expect("safe leases");
    let selected = (previous[0].0 + 1, previous[0].1);
    let metadata = leases
        .each_ref()
        .map(|file| file.metadata().expect("metadata"));
    let mut changes = Vec::new();
    rebind_user_leases(&leases, &previous, selected, |file, owner| {
        assert!(matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)));
        assert!(matches!(
            directory.acquire(),
            Err(crate::LeaseError::Busy { .. })
        ));
        changes.push(owner);
        Ok(())
    })
    .expect("simulated account rebind");
    assert_eq!(changes, [selected; 2]);
    for (index, file) in leases.iter().enumerate() {
        let after = file.metadata().expect("after metadata");
        assert_eq!(
            (metadata[index].dev(), metadata[index].ino()),
            (after.dev(), after.ino())
        );
        assert_eq!(after.mode() & 0o7777, 0o644);
        assert_eq!(
            fs::read(root.join(USER_LEASE_NAMES[index])).expect("content"),
            USER_LEASE_NAMES[index].as_bytes()
        );
    }
    assert_eq!(
        fs::read(root.join(INSTALLER_LEASE_NAME)).expect("installer content"),
        INSTALLER_LEASE_NAME.as_bytes()
    );
    drop(guard);
    drop(directory.acquire().expect("all locks released"));
}

#[test]
fn same_account_keeps_existing_owners_without_a_change_call() {
    let (_root, leases) = lease_fixture("lease-same-account");
    let previous = inspect_user_lease_owners(&leases).expect("owners");
    rebind_user_leases(&leases, &previous, previous[0], |_, _| {
        panic!("unchanged ownership must not invoke fchown")
    })
    .expect("same account");
}

#[test]
fn only_the_lease_whose_owner_differs_is_changed() {
    let (_root, leases) = lease_fixture("lease-one-differs");
    let current = inspect_user_lease_owners(&leases).expect("owners");
    let other = (current[0].0 + 1, current[0].1);
    let mut changed = Vec::new();
    rebind_user_leases(&leases, &[other, current[1]], current[1], |file, owner| {
        changed.push((file.metadata().expect("metadata").ino(), owner));
        Ok(())
    })
    .expect("rebind");
    let app = leases[0].metadata().expect("app metadata").ino();
    assert_eq!(changed, [(app, current[1])]);
}

#[test]
fn every_active_owner_prevents_the_rebinding_guard() {
    for name in USER_LEASE_NAMES.into_iter().chain([INSTALLER_LEASE_NAME]) {
        let (root, leases) = lease_fixture("lease-rebind-busy");
        let owner = File::options()
            .read(true)
            .write(true)
            .open(root.join(name))
            .expect("active owner descriptor");
        owner.try_lock().expect("active owner");
        let before = inspect_user_lease_owners(&leases).expect("owners");
        let directory = LifecycleDirectory::open(root.path()).expect("directory");
        assert!(matches!(
            directory.acquire(),
            Err(crate::LeaseError::Busy { .. })
        ));
        assert_eq!(
            inspect_user_lease_owners(&leases).expect("retained owners"),
            before
        );
        // Runtime owners explicitly unlock on Drop, before inherited FDs close.
        owner.unlock().expect("release active owner lease");
        drop(owner);
        drop(directory.acquire().expect("released owner"));
    }
}

#[test]
fn any_retained_install_target_prevents_account_rebinding() {
    for target in OwnedTarget::all() {
        let root = FixtureDir::new("lease-retained-installation").expect("fixture");
        require_absent_targets(&root).expect("fully removed");
        let path = root.join(target.path().trim_start_matches('/'));
        fs::create_dir_all(path.parent().expect("target parent")).expect("parent");
        fs::write(&path, b"installed").expect("retained target");
        assert!(matches!(require_absent_targets(&root),
            Err(NativeError::Trust { path: rejected, .. }) if rejected == path));
    }
}

#[test]
fn unsafe_existing_leases_are_rejected_before_account_changes() {
    let (root, leases) = lease_fixture("lease-unsafe-mode");
    fs::set_permissions(root.join("app.owner"), fs::Permissions::from_mode(0o600))
        .expect("wrong mode");
    assert!(inspect_user_lease_owners(&leases).is_err());
    fs::set_permissions(root.join("app.owner"), fs::Permissions::from_mode(0o644))
        .expect("restore mode");
    fs::hard_link(root.join("app.owner"), root.join("alias")).expect("second link");
    assert!(inspect_user_lease_owners(&leases).is_err());
    let (root, _leases) = lease_fixture("lease-unsafe-symlink");
    fs::remove_file(root.join("app.owner")).expect("fixture path");
    symlink("engine.owner", root.join("app.owner")).expect("symlink");
    let directory = LifecycleDirectory::open(root.path()).expect("directory");
    assert!(directory.acquire().is_err());
}

#[test]
fn partial_owner_change_failure_restores_earlier_owners() {
    let (_root, leases) = lease_fixture("lease-rebind-restore");
    let previous = inspect_user_lease_owners(&leases).expect("owners");
    let selected = (previous[0].0 + 1, previous[0].1);
    let mut changes = Vec::new();
    let result = rebind_user_leases(&leases, &previous, selected, |_, owner| {
        changes.push(owner);
        if changes.len() == 2 {
            Err(io::Error::other("fixture chown failed"))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert_eq!(changes, [selected, selected, previous[0]]);
}

#[test]
fn restore_failures_report_each_path_and_continue_restoring_other_owners() {
    let (_root, leases) = lease_fixture("lease-rebind-restore-failed");
    let previous = inspect_user_lease_owners(&leases).expect("owners");
    let selected = (previous[0].0 + 1, previous[0].1);
    let mut calls = 0;
    let error = rebind_user_leases(&leases, &previous, selected, |_, _| {
        calls += 1;
        if calls >= 2 {
            Err(io::Error::other("fixture owner change failure"))
        } else {
            Ok(())
        }
    })
    .expect_err("failed change and restoration");
    assert_eq!(calls, 3);
    let detail = error.to_string();
    for name in USER_LEASE_NAMES {
        assert!(
            detail.contains(name),
            "missing failure path {name}: {detail}"
        );
    }
    assert!(detail.contains("restoration failed"));
}
