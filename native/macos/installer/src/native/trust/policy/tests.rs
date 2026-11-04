use std::path::Path;

use super::*;

const WHEEL: u32 = 0;
const ADMIN: u32 = 80;
const STAFF: u32 = 20;

fn directory(uid: u32, gid: u32, mode: u32) -> Inode {
    Inode {
        kind: InodeKind::Directory,
        uid,
        gid,
        mode,
        links: 2,
        len: 64,
    }
}

fn file(uid: u32, gid: u32, mode: u32) -> Inode {
    Inode {
        kind: InodeKind::File,
        uid,
        gid,
        mode,
        links: 1,
        len: 1024,
    }
}

fn with_kind(inode: Inode, kind: InodeKind) -> Inode {
    Inode { kind, ..inode }
}

#[test]
fn applications_group_writable_exception_is_pinned_to_exact_policy() {
    assert_eq!(ROOT_DIRECTORY_MODE, 0o755);
    assert_eq!(LEASE_FILE_MODE, 0o644);
    assert_eq!(
        super::super::APP_DIRECTORY,
        "/Library/Application Support/MixRelay"
    );
    assert!(applications_directory(directory(0, ADMIN, 0o775), ADMIN).is_ok());
    assert!(applications_directory(directory(0, ADMIN, 0o755), ADMIN).is_ok());
    assert!(applications_directory(directory(501, ADMIN, 0o775), ADMIN).is_err());
    assert!(applications_directory(directory(0, ADMIN, 0o777), ADMIN).is_err());
    // Group write is accepted only for the admin group, not any other group.
    assert!(applications_directory(directory(0, STAFF, 0o775), ADMIN).is_err());
}

#[test]
fn walked_components_must_be_root_directories_closed_to_other_writers() {
    let path = Path::new("/Library");
    assert!(path_component(path, directory(0, WHEEL, 0o755)).is_ok());
    for unsafe_component in [
        directory(501, WHEEL, 0o755),
        directory(0, WHEEL, 0o775),
        directory(0, WHEEL, 0o757),
        with_kind(directory(0, WHEEL, 0o755), InodeKind::Symlink),
        file(0, WHEEL, 0o755),
    ] {
        assert!(path_component(path, unsafe_component).is_err());
    }
    // /Applications defers to its own rule, which the destination check applies.
    assert!(path_component(Path::new("/Applications"), directory(0, ADMIN, 0o775)).is_ok());
}

#[test]
fn destination_parents_reject_foreign_owners_and_shared_write_access() {
    let protected = ParentRule::Protected;
    let applications = ParentRule::Applications { admin_gid: ADMIN };
    assert!(destination_parent(directory(0, WHEEL, 0o755), protected).is_ok());
    assert!(destination_parent(directory(0, ADMIN, 0o775), applications).is_ok());
    assert_eq!(
        destination_parent(directory(0, ADMIN, 0o775), protected),
        Err("destination parent must not be group/other writable")
    );
    assert_eq!(
        destination_parent(directory(501, WHEEL, 0o755), applications),
        Err("destination parent must be a root-owned directory")
    );
    assert_eq!(
        destination_parent(file(0, WHEEL, 0o755), protected),
        Err("destination parent must be a root-owned directory")
    );
}

#[test]
fn created_directories_require_root_the_expected_group_and_exactly_0755() {
    assert!(root_directory(directory(0, WHEEL, 0o755), Some(WHEEL)).is_ok());
    assert!(root_directory(directory(0, STAFF, 0o755), None).is_ok());
    for rejected in [
        directory(0, STAFF, 0o755),
        directory(0, WHEEL, 0o750),
        directory(0, WHEEL, 0o775),
        directory(501, WHEEL, 0o755),
        file(0, WHEEL, 0o755),
    ] {
        assert!(root_directory(rejected, Some(WHEEL)).is_err());
    }
    let link = with_kind(directory(0, WHEEL, 0o755), InodeKind::Symlink);
    assert_eq!(
        engine_directory(link, WHEEL),
        Err("engine parent must be a direct root:wheel 0755 directory")
    );
    assert!(engine_directory(directory(0, WHEEL, 0o755), WHEEL).is_ok());
}

#[test]
fn owned_targets_must_be_direct_root_wheel_inodes_with_their_fixed_mode() {
    assert!(owned_target(file(0, 0, 0o755), 0o755).is_ok());
    assert!(owned_target(directory(0, 0, 0o755), 0o755).is_ok());
    assert_eq!(
        owned_target(file(0, 0, 0o775), 0o755),
        Err("owned target mode differs from the fixed distribution policy")
    );
    for foreign in [
        file(501, 0, 0o755),
        file(0, STAFF, 0o755),
        with_kind(file(0, 0, 0o755), InodeKind::Symlink),
    ] {
        assert_eq!(
            owned_target(foreign, 0o755),
            Err("owned target must be a direct root:wheel inode")
        );
    }
}

#[test]
fn leases_are_single_link_regular_0644_files() {
    assert!(lease(file(501, STAFF, 0o644)).is_ok());
    assert!(lease(file(0, WHEEL, 0o644)).is_ok());
    for rejected in [
        file(501, STAFF, 0o600),
        file(501, STAFF, 0o666),
        Inode {
            links: 2,
            ..file(501, STAFF, 0o644)
        },
        with_kind(file(501, STAFF, 0o644), InodeKind::Symlink),
        with_kind(file(501, STAFF, 0o644), InodeKind::Other),
    ] {
        assert!(lease(rejected).is_err());
    }
}

#[test]
fn a_root_owned_retained_lease_never_names_the_service_account() {
    assert!(retained_account_lease(file(501, STAFF, 0o644)).is_ok());
    assert!(retained_account_lease(file(0, WHEEL, 0o644)).is_err());
    assert!(retained_account_lease(file(501, STAFF, 0o600)).is_err());
}

#[test]
fn installed_plist_must_be_a_bounded_root_wheel_0644_file() {
    let plist = file(0, WHEEL, 0o644);
    assert!(installed_launchd_plist(plist, WHEEL).is_ok());
    let at_limit = Inode {
        len: MAX_LAUNCHD_PLIST_BYTES,
        ..plist
    };
    assert!(installed_launchd_plist(at_limit, WHEEL).is_ok());
    for rejected in [
        Inode {
            len: MAX_LAUNCHD_PLIST_BYTES + 1,
            ..plist
        },
        Inode { links: 2, ..plist },
        file(501, WHEEL, 0o644),
        file(0, STAFF, 0o644),
        file(0, WHEEL, 0o664),
        with_kind(plist, InodeKind::Symlink),
    ] {
        assert!(installed_launchd_plist(rejected, WHEEL).is_err());
    }
}
