//! Root-owned lifecycle directory and its fixed lease inodes.
//!
//! Existing inodes are never replaced or chmodded. The two user leases carry the
//! service account as their owner, so they also identify an installation whose
//! launchd plist is gone.

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

use rustix::{
    fs::{Gid, Mode, OFlags, Uid, fchmod, fchown, openat},
    io::Errno,
};

use super::{
    APP_DIRECTORY,
    anchored::{
        errno_error, inspect, io_error, open_child_directory, open_or_create_root_directory,
        open_protected_directory, open_root, require_root_directory,
    },
    destination::validate_destination_ancestors,
    policy::{self, LEASE_FILE_MODE},
    require, wheel_group,
};
use crate::{
    LIFECYCLE_DIRECTORY, LifecycleDirectory, OwnedTarget,
    native::{
        NativeError, ResolvedServiceAccount,
        account::{resolve_service_account, resolve_service_uid},
        launchctl::{self, RegistrationState},
        privilege::require_root,
    },
};

const APP_LEASE_NAME: &str = "app.owner";
const ENGINE_LEASE_NAME: &str = "engine.owner";
const INSTALLER_LEASE_NAME: &str = "installer.owner";
const USER_LEASE_NAMES: [&str; 2] = [APP_LEASE_NAME, ENGINE_LEASE_NAME];

/// A lease inode's `(uid, gid)`.
type Owner = (u32, u32);

/// Creates only `MixRelay`'s root-owned lifecycle directory and missing fixed leases.
///
/// Existing inodes are never replaced or chmodded. After complete removal, idle
/// user leases may be rebound to the new installation account under all lifecycle
/// locks. Failures leave newly created infrastructure available for administrator retry.
/// # Errors
/// Returns when ownership, mode, inode type, or fixed path validation fails.
pub fn initialize_lifecycle_storage(account: &ResolvedServiceAccount) -> Result<(), NativeError> {
    require_root()?;
    require_unchanged_account(
        account,
        "selected service account changed before lifecycle initialization",
    )?;
    let wheel = wheel_group()?;
    let lifecycle = open_or_create_lifecycle_directory(wheel)?;
    let owner = (account.uid(), account.gid());
    let leases = [
        ensure_lease_file(&lifecycle, APP_LEASE_NAME, owner)?,
        ensure_lease_file(&lifecycle, ENGINE_LEASE_NAME, owner)?,
    ];
    let installer = (0, wheel.as_raw());
    ensure_lease_file(&lifecycle, INSTALLER_LEASE_NAME, installer)?;
    validate_existing_lease(&lifecycle, INSTALLER_LEASE_NAME, installer)?;
    let directory = LifecycleDirectory::open(LIFECYCLE_DIRECTORY)?;
    // Shared process locks exclude their exclusive owners; installer exclusivity
    // serializes this ownership change with every install/remove transaction.
    let previous = inspect_user_lease_owners(&leases)?;
    let _guard = directory.acquire()?;
    if previous.iter().any(|current| *current != owner) {
        require_removed_distribution()?;
        rebind_user_leases(&leases, &previous, owner, change_lease_owner)?;
    }
    require_lifecycle_storage(account)?;
    require_unchanged_account(
        account,
        "selected service account changed during lifecycle initialization",
    )
}

/// Revalidates the fixed lifecycle directory and every no-replace lease inode.
/// # Errors
/// Returns when the fixed directory or any lease inode violates its ownership,
/// link-count, type, or mode policy.
pub fn require_lifecycle_storage(account: &ResolvedServiceAccount) -> Result<(), NativeError> {
    let wheel = wheel_group()?;
    let directory = open_lifecycle_directory(wheel)?;
    let owner = (account.uid(), account.gid());
    validate_existing_lease(&directory, APP_LEASE_NAME, owner)?;
    validate_existing_lease(&directory, ENGINE_LEASE_NAME, owner)?;
    validate_existing_lease(&directory, INSTALLER_LEASE_NAME, (0, wheel.as_raw()))
}

/// Missing plist fallback: root-owned lifecycle parents retain the original
/// service user's fixed lease inodes. Missing individual leases are recreated
/// later; present untrusted or inconsistent leases never select another user.
pub(in crate::native) fn read_lifecycle_account()
-> Result<Option<ResolvedServiceAccount>, NativeError> {
    let path = Path::new(LIFECYCLE_DIRECTORY);
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error("inspect retained lifecycle account", path, source)),
    }
    let directory = open_lifecycle_directory(wheel_group()?)?;
    let mut account = None;
    for name in USER_LEASE_NAMES {
        let Some(resolved) = read_retained_lease_account(&directory, name)? else {
            continue;
        };
        if account
            .as_ref()
            .is_some_and(|previous| previous != &resolved)
        {
            return Err(NativeError::Trust {
                path: lease_path(name),
                reason: "retained service account leases disagree",
            });
        }
        account = Some(resolved);
    }
    Ok(account)
}

fn read_retained_lease_account(
    directory: &File,
    name: &str,
) -> Result<Option<ResolvedServiceAccount>, NativeError> {
    let path = lease_path(name);
    let file = match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => File::from(descriptor),
        Err(Errno::NOENT) => return Ok(None),
        Err(source) => return Err(errno_error("open retained account lease", &path, source)),
    };
    let inode = inspect(&file, "inspect retained account lease", &path)?;
    require(&path, policy::retained_account_lease(inode))?;
    let resolved = resolve_service_uid(inode.uid)?;
    if inode.gid != resolved.gid() {
        return Err(NativeError::Trust {
            path,
            reason: "retained service account leases disagree",
        });
    }
    Ok(Some(resolved))
}

fn require_unchanged_account(
    account: &ResolvedServiceAccount,
    message: &str,
) -> Result<(), NativeError> {
    if &resolve_service_account(account.account())? == account {
        Ok(())
    } else {
        Err(NativeError::Account(message.to_owned()))
    }
}

/// Walks `/Library/Application Support/MixRelay/lifecycle`, creating the product levels.
fn open_or_create_lifecycle_directory(wheel: Gid) -> Result<File, NativeError> {
    let library_path = Path::new("/Library");
    let library = open_child_directory(&open_root()?, "Library", library_path)?;
    require_root_directory(library_path, &library, None)?;
    let support_path = Path::new("/Library/Application Support");
    let support = open_child_directory(&library, "Application Support", support_path)?;
    require_root_directory(support_path, &support, None)?;
    let app_path = Path::new(APP_DIRECTORY);
    let app = open_or_create_root_directory(&support, "MixRelay", app_path, wheel)?;
    require_root_directory(app_path, &app, Some(wheel))?;
    let lifecycle_path = Path::new(LIFECYCLE_DIRECTORY);
    let lifecycle = open_or_create_root_directory(&app, "lifecycle", lifecycle_path, wheel)?;
    require_root_directory(lifecycle_path, &lifecycle, Some(wheel))?;
    Ok(lifecycle)
}

fn open_lifecycle_directory(wheel: Gid) -> Result<File, NativeError> {
    let path = Path::new(LIFECYCLE_DIRECTORY);
    let directory = open_protected_directory(path)?;
    require_root_directory(path, &directory, Some(wheel))?;
    Ok(directory)
}

fn lease_path(name: &str) -> PathBuf {
    Path::new(LIFECYCLE_DIRECTORY).join(name)
}

/// Opens a lease, creating it exclusively with `owner` and 0644 when missing.
fn ensure_lease_file(directory: &File, name: &str, owner: Owner) -> Result<File, NativeError> {
    let path = lease_path(name);
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let mode = Mode::from_raw_mode(LEASE_FILE_MODE);
    match openat(directory, name, flags | OFlags::CREATE | OFlags::EXCL, mode) {
        Ok(descriptor) => {
            let file = File::from(descriptor);
            change_lease_owner(&file, owner)
                .map_err(|source| io_error("set lifecycle lease ownership", &path, source))?;
            fchmod(&file, mode)
                .map_err(|source| errno_error("set lifecycle lease mode", &path, source))?;
            inspect_lease_owner(&file, name)?;
            Ok(file)
        }
        Err(Errno::EXIST) => {
            let file = openat(directory, name, flags, Mode::empty())
                .map(File::from)
                .map_err(|source| errno_error("open existing lifecycle lease", &path, source))?;
            inspect_lease_owner(&file, name)?;
            Ok(file)
        }
        Err(source) => Err(errno_error("create lifecycle lease", &path, source)),
    }
}

fn inspect_lease_owner(file: &File, name: &str) -> Result<Owner, NativeError> {
    let path = lease_path(name);
    let inode = inspect(file, "inspect lifecycle lease", &path)?;
    require(&path, policy::lease(inode))?;
    Ok((inode.uid, inode.gid))
}

fn validate_existing_lease(directory: &File, name: &str, owner: Owner) -> Result<(), NativeError> {
    let path = lease_path(name);
    let file = openat(
        directory,
        name,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|source| errno_error("open fixed lifecycle lease", &path, source))?;
    if inspect_lease_owner(&file, name)? != owner {
        return Err(NativeError::Trust {
            path,
            reason: "fixed lifecycle lease inode has unexpected owner, links, type or mode",
        });
    }
    Ok(())
}

/// A lease account changes only when nothing of the previous installation remains.
fn require_removed_distribution() -> Result<(), NativeError> {
    validate_destination_ancestors(Path::new("/"))?;
    require_absent_targets(Path::new("/"))?;
    if launchctl::inspect(&launchctl::DRIVER)? != RegistrationState::Absent {
        return Err(NativeError::LaunchdState(
            "lease account can change only when the Driver registration is absent",
        ));
    }
    Ok(())
}

fn require_absent_targets(root: &Path) -> Result<(), NativeError> {
    for target in OwnedTarget::all() {
        let path = root.join(target.path().trim_start_matches('/'));
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(NativeError::Trust {
                    path,
                    reason: "lease account can change only after complete distribution removal",
                });
            }
            Err(source) => {
                return Err(io_error(
                    "inspect distribution before rebinding lifecycle leases",
                    &path,
                    source,
                ));
            }
        }
    }
    Ok(())
}

fn change_lease_owner(file: &File, owner: Owner) -> io::Result<()> {
    fchown(
        file,
        Some(Uid::from_raw(owner.0)),
        Some(Gid::from_raw(owner.1)),
    )
    .map_err(io::Error::from)
}

fn inspect_user_lease_owners(leases: &[File; 2]) -> Result<[Owner; 2], NativeError> {
    Ok([
        inspect_lease_owner(&leases[0], USER_LEASE_NAMES[0])?,
        inspect_lease_owner(&leases[1], USER_LEASE_NAMES[1])?,
    ])
}

/// Changes each differing user lease to `owner`; on failure, restores the earlier ones.
fn rebind_user_leases(
    leases: &[File; 2],
    previous: &[Owner; 2],
    owner: Owner,
    mut change_owner: impl FnMut(&File, Owner) -> io::Result<()>,
) -> Result<(), NativeError> {
    for (index, lease) in leases.iter().enumerate() {
        if previous[index] == owner {
            continue;
        }
        if let Err(source) = change_owner(lease, owner) {
            let path = lease_path(USER_LEASE_NAMES[index]);
            let cause = io_error("rebind lifecycle lease owner", &path, source);
            let restore_errors =
                restore_lease_owners(leases, previous, owner, index, &mut change_owner);
            if restore_errors.is_empty() {
                return Err(cause);
            }
            return Err(io_error(
                "restore lifecycle lease owners after failed account change",
                &path,
                io::Error::other(format!("{cause}; restoration failed: {restore_errors:?}")),
            ));
        }
    }
    Ok(())
}

/// Restores leases before `failed_index` in reverse order, continuing past failures.
fn restore_lease_owners(
    leases: &[File; 2],
    previous: &[Owner; 2],
    owner: Owner,
    failed_index: usize,
    change_owner: &mut impl FnMut(&File, Owner) -> io::Result<()>,
) -> Vec<NativeError> {
    (0..failed_index)
        .rev()
        .filter(|index| previous[*index] != owner)
        .filter_map(|index| {
            change_owner(&leases[index], previous[index])
                .err()
                .map(|source| {
                    io_error(
                        "restore lifecycle lease owner",
                        &lease_path(USER_LEASE_NAMES[index]),
                        source,
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests;
