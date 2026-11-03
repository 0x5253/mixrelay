//! Descriptor-anchored root-owned directory and lifecycle-inode initialization.

use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use rustix::{
    fs::{Gid, Mode, OFlags, Uid, fchmod, fchown, mkdirat, open, openat},
    io::Errno,
};

use crate::{
    LAUNCHD_PATH, LIFECYCLE_DIRECTORY, LifecycleDirectory, OwnedTarget, PRODUCT_DIRECTORY,
};

use super::{
    NativeError, ResolvedServiceAccount,
    account::{group_id_for_name, resolve_service_account},
    launchctl::{self, RegistrationState},
};

const APP_DIRECTORY: &str = "/Library/Application Support/MixRelay";
const ROOT_DIRECTORY_MODE: u16 = 0o755;
const LEASE_FILE_MODE: u16 = 0o644;
const APP_LEASE_NAME: &str = "app.owner";
const ENGINE_LEASE_NAME: &str = "engine.owner";
const INSTALLER_LEASE_NAME: &str = "installer.owner";
const USER_LEASE_NAMES: [&str; 2] = [APP_LEASE_NAME, ENGINE_LEASE_NAME];
const MAX_LAUNCHD_PLIST_BYTES: u64 = 16 * 1024;

/// Refuses to perform any native privileged work unless already running as root.
///
/// The installer never invokes `sudo`, prompts for a password, or changes its
/// effective identity.
/// # Errors
/// Returns [`NativeError::RootRequired`] unless the effective identity is root:wheel.
pub fn require_root() -> Result<(), NativeError> {
    // SAFETY: geteuid is a read-only process identity query.
    let effective_user_id = unsafe { libc::geteuid() };
    if effective_user_id != 0 {
        return Err(NativeError::RootRequired);
    }
    let wheel = group_id_for_name("wheel")?;
    // SAFETY: getegid is a read-only process identity query.
    let effective_group_id = unsafe { libc::getegid() };
    if root_wheel_identity(effective_user_id, effective_group_id, wheel) {
        Ok(())
    } else {
        Err(NativeError::RootRequired)
    }
}

fn root_wheel_identity(user_id: u32, group_id: u32, wheel_gid: u32) -> bool {
    user_id == 0 && group_id == wheel_gid
}

/// Creates only `MixRelay`'s root-owned lifecycle directory and missing fixed leases.
///
/// Existing inodes are never replaced or chmodded. After complete removal, idle
/// user leases may be rebound to the new installation account under all lifecycle
/// locks. Failures leave newly created infrastructure available for administrator retry.
/// # Errors
/// Returns when ownership, mode, inode type, or fixed path validation fails.
pub fn initialize_lifecycle_storage(account: &ResolvedServiceAccount) -> Result<(), NativeError> {
    require_root()?;
    if &resolve_service_account(account.account())? != account {
        return Err(NativeError::Account(
            "selected service account changed before lifecycle initialization".to_owned(),
        ));
    }
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    let root = open_root()?;
    let library = open_existing_child(&root, "Library", Path::new("/Library"))?;
    validate_root_directory(Path::new("/Library"), &library, None)?;
    let support = open_existing_child(
        &library,
        "Application Support",
        Path::new("/Library/Application Support"),
    )?;
    validate_root_directory(Path::new("/Library/Application Support"), &support, None)?;
    let app_directory =
        open_or_create_child(&support, "MixRelay", Path::new(APP_DIRECTORY), wheel)?;
    validate_root_directory(Path::new(APP_DIRECTORY), &app_directory, Some(wheel))?;
    let lifecycle = open_or_create_child(
        &app_directory,
        "lifecycle",
        Path::new(LIFECYCLE_DIRECTORY),
        wheel,
    )?;
    validate_root_directory(Path::new(LIFECYCLE_DIRECTORY), &lifecycle, Some(wheel))?;

    let leases = [
        ensure_lease_file(&lifecycle, APP_LEASE_NAME, account.uid(), account.gid())?,
        ensure_lease_file(&lifecycle, ENGINE_LEASE_NAME, account.uid(), account.gid())?,
    ];
    ensure_lease_file(&lifecycle, INSTALLER_LEASE_NAME, 0, wheel.as_raw())?;
    validate_existing_lease(&lifecycle, INSTALLER_LEASE_NAME, 0, wheel.as_raw())?;
    let directory = LifecycleDirectory::open(LIFECYCLE_DIRECTORY)?;
    // Shared process locks exclude their exclusive owners; installer exclusivity
    // serializes this ownership change with every install/remove transaction.
    let previous = inspect_user_lease_owners(&leases)?;
    let owner = (account.uid(), account.gid());
    let _guard = directory.acquire()?;
    if previous.iter().any(|current| *current != owner) {
        require_removed_distribution()?;
        rebind_user_leases(&leases, &previous, owner, change_lease_owner)?;
    }
    require_lifecycle_storage(account)?;
    if &resolve_service_account(account.account())? != account {
        return Err(NativeError::Account(
            "selected service account changed during lifecycle initialization".to_owned(),
        ));
    }
    Ok(())
}

/// Creates the system HAL plug-in directory when it is missing. A fresh macOS has no
/// `/Library/Audio/Plug-Ins/HAL` until some audio driver installs one. Missing levels are
/// created as root:wheel 0755, as macOS does; existing ones are left unchanged and
/// `validate_destination_ancestors` then rejects unsafe ownership or modes.
/// # Errors
/// Returns when a level cannot be opened or created without following links.
pub fn prepare_install_directories() -> Result<(), NativeError> {
    require_root()?;
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    let root = open_root()?;
    let library = open_existing_child(&root, "Library", Path::new("/Library"))?;
    let audio = open_or_create_child(&library, "Audio", Path::new("/Library/Audio"), wheel)?;
    let plugins = open_or_create_child(
        &audio,
        "Plug-Ins",
        Path::new("/Library/Audio/Plug-Ins"),
        wheel,
    )?;
    open_or_create_child(
        &plugins,
        "HAL",
        Path::new("/Library/Audio/Plug-Ins/HAL"),
        wheel,
    )?;
    Ok(())
}

mod product;

/// Publishes a privately configured product folder into Applications exclusively.
pub(crate) fn create_product_directory() -> Result<File, NativeError> {
    require_root()?;
    let path = Path::new(PRODUCT_DIRECTORY);
    let applications_path = path.parent().ok_or_else(|| NativeError::Trust {
        path: path.to_path_buf(),
        reason: "product folder has no Applications parent",
    })?;
    let applications = open_existing_directory(applications_path)?;
    let metadata = applications.metadata().map_err(|source| NativeError::Io {
        operation: "inspect Applications directory",
        path: applications_path.to_path_buf(),
        source,
    })?;
    if !applications_directory_policy(
        metadata.uid(),
        metadata.gid(),
        metadata.mode() & 0o7777,
        group_id_for_name("admin")?,
    ) {
        return Err(NativeError::Trust {
            path: applications_path.to_path_buf(),
            reason: "unsafe Applications directory",
        });
    }
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    let lifecycle = open_existing_directory(Path::new(LIFECYCLE_DIRECTORY))?;
    validate_root_directory(Path::new(LIFECYCLE_DIRECTORY), &lifecycle, Some(wheel))?;
    let directory =
        product::create_product_directory_at(&applications, &lifecycle, (Uid::ROOT, wheel), || {})
            .map_err(|source| NativeError::Io {
                operation: "publish protected product folder",
                path: path.to_path_buf(),
                source,
            })?;
    validate_root_directory(path, &directory, Some(wheel))?;
    Ok(directory)
}

/// Revalidates the fixed lifecycle directory and every no-replace lease inode.
/// # Errors
/// Returns when the fixed directory or any lease inode violates its ownership,
/// link-count, type, or mode policy.
pub fn require_lifecycle_storage(account: &ResolvedServiceAccount) -> Result<(), NativeError> {
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    let directory = open_existing_directory(Path::new(LIFECYCLE_DIRECTORY))?;
    validate_root_directory(Path::new(LIFECYCLE_DIRECTORY), &directory, Some(wheel))?;
    validate_existing_lease(&directory, APP_LEASE_NAME, account.uid(), account.gid())?;
    validate_existing_lease(&directory, ENGINE_LEASE_NAME, account.uid(), account.gid())?;
    validate_existing_lease(&directory, INSTALLER_LEASE_NAME, 0, wheel.as_raw())?;
    Ok(())
}

/// Missing plist fallback: root-owned lifecycle parents retain the original
/// service user's fixed lease inodes. Missing individual leases are recreated
/// later; present untrusted or inconsistent leases never select another user.
pub(super) fn read_lifecycle_account() -> Result<Option<ResolvedServiceAccount>, NativeError> {
    let path = Path::new(LIFECYCLE_DIRECTORY);
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(NativeError::Io {
                operation: "inspect retained lifecycle account",
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let directory = open_existing_directory(path)?;
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    validate_root_directory(path, &directory, Some(wheel))?;
    let mut account = None;
    for name in USER_LEASE_NAMES {
        let descriptor = match openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::NOENT) => continue,
            Err(source) => {
                return Err(NativeError::Io {
                    operation: "open retained account lease",
                    path: path.join(name),
                    source: source.into(),
                });
            }
        };
        let metadata = File::from(descriptor)
            .metadata()
            .map_err(|source| NativeError::Io {
                operation: "inspect retained account lease",
                path: path.join(name),
                source,
            })?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() == 0
            || metadata.mode() & 0o7777 != u32::from(LEASE_FILE_MODE)
        {
            return Err(NativeError::Trust {
                path: path.join(name),
                reason: "invalid retained service account lease",
            });
        }
        let resolved = super::account::resolve_service_uid(metadata.uid())?;
        if metadata.gid() != resolved.gid()
            || account
                .as_ref()
                .is_some_and(|previous| previous != &resolved)
        {
            return Err(NativeError::Trust {
                path: path.join(name),
                reason: "retained service account leases disagree",
            });
        }
        account = Some(resolved);
    }
    Ok(account)
}

/// Reads the fixed installed launchd plist through a trusted directory descriptor.
/// Missing is reported only for an actual `ENOENT`; malformed or unsafe objects
/// remain errors so callers never fall back to a different console identity.
pub(super) fn read_installed_launchd_plist() -> Result<Option<Vec<u8>>, NativeError> {
    let path = Path::new(LAUNCHD_PATH);
    let parent_path = Path::new("/Library/LaunchDaemons");
    let parent = open_existing_directory(parent_path)?;
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    let descriptor = match openat(
        &parent,
        "io.mixrelay.audio.driver.plist",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => return Ok(None),
        Err(source) => {
            return Err(NativeError::Io {
                operation: "open installed launchd plist",
                path: path.to_path_buf(),
                source: io::Error::from(source),
            });
        }
    };
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|source| NativeError::Io {
        operation: "inspect installed launchd plist",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != 0
        || metadata.gid() != wheel.as_raw()
        || metadata.mode() & 0o7777 != 0o644
        || metadata.len() > MAX_LAUNCHD_PLIST_BYTES
    {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "installed launchd plist must be a bounded root:wheel 0644 regular inode",
        });
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| NativeError::Trust {
        path: path.to_path_buf(),
        reason: "installed launchd plist size exceeds the supported address space",
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_LAUNCHD_PLIST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| NativeError::Io {
            operation: "read installed launchd plist",
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > MAX_LAUNCHD_PLIST_BYTES {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "installed launchd plist exceeds its fixed size limit",
        });
    }
    Ok(Some(bytes))
}

/// Revalidates every existing directory leading to an install target without changing it.
pub(crate) fn validate_destination_ancestors(root: &Path) -> Result<(), NativeError> {
    if root != Path::new("/") {
        return Err(NativeError::Trust {
            path: root.to_path_buf(),
            reason: "native service operations require the production filesystem root",
        });
    }
    let admin = group_id_for_name("admin")?;
    let fixed = [
        (Path::new("/Applications"), true),
        (Path::new("/Library/Application Support"), false),
        (Path::new(APP_DIRECTORY), false),
        (Path::new("/Library/Audio/Plug-Ins/HAL"), false),
        (Path::new("/Library/LaunchDaemons"), false),
    ];
    for (directory, applications_policy) in fixed {
        let opened = open_existing_directory(directory)?;
        let metadata = opened.metadata().map_err(|source| NativeError::Io {
            operation: "inspect destination parent",
            path: directory.to_path_buf(),
            source,
        })?;
        if metadata.uid() != 0 || !metadata.is_dir() {
            return Err(NativeError::Trust {
                path: directory.to_path_buf(),
                reason: "destination parent must be a root-owned directory",
            });
        }
        let mode = metadata.mode() & 0o7777;
        if applications_policy {
            // Preserve macOS's normal root:admin 0775 Applications policy. Other
            // equally restrictive root-owned modes are accepted without rewrite.
            if !applications_directory_policy(metadata.uid(), metadata.gid(), mode, admin) {
                return Err(NativeError::Trust {
                    path: directory.to_path_buf(),
                    reason: "Applications must retain root:admin 0775 or a restrictive root-owned mode",
                });
            }
        } else if mode & 0o022 != 0 {
            return Err(NativeError::Trust {
                path: directory.to_path_buf(),
                reason: "destination parent must not be group/other writable",
            });
        }
    }
    let product = Path::new(PRODUCT_DIRECTORY);
    match std::fs::symlink_metadata(product) {
        Ok(_) => {
            open_existing_directory(product)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(NativeError::Io {
                operation: "inspect product folder",
                path: product.to_path_buf(),
                source,
            });
        }
    }
    validate_engine_directory(root, false)?;
    Ok(())
}

fn applications_directory_policy(uid: u32, gid: u32, mode: u32, admin_gid: u32) -> bool {
    uid == 0 && ((gid == admin_gid && mode == 0o775) || mode & 0o022 == 0)
}

/// Checks the one engine parent, allowing it to be absent only during preflight.
pub(super) fn validate_engine_directory(root: &Path, required: bool) -> Result<(), NativeError> {
    let path = root.join("Library/Application Support/MixRelay/bin");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(NativeError::Io {
                operation: "inspect engine parent",
                path,
                source,
            });
        }
    };
    let wheel = Gid::from_raw(group_id_for_name("wheel")?);
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != 0
        || metadata.gid() != wheel.as_raw()
        || metadata.mode() & 0o7777 != u32::from(ROOT_DIRECTORY_MODE)
    {
        return Err(NativeError::Trust {
            path,
            reason: "engine parent must be a direct root:wheel 0755 directory",
        });
    }
    Ok(())
}

/// Requires exact root ownership and closed permissions on one existing target.
pub(super) fn validate_owned_target(path: &Path, mode: u32) -> Result<(), NativeError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| NativeError::Io {
        operation: "inspect owned target",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || metadata.uid() != 0 || metadata.gid() != 0 {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "owned target must be a direct root:wheel inode",
        });
    }
    if metadata.mode() & 0o7777 != mode {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "owned target mode differs from the fixed distribution policy",
        });
    }
    Ok(())
}

fn open_root() -> Result<File, NativeError> {
    let descriptor = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| NativeError::Io {
        operation: "open filesystem root",
        path: PathBuf::from("/"),
        source: io::Error::from(source),
    })?;
    Ok(File::from(descriptor))
}

fn open_existing_directory(path: &Path) -> Result<File, NativeError> {
    validate_absolute_normalized(path)?;
    let mut current = open_root()?;
    let mut walked = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(name) = component {
            walked.push(name);
            current = open_existing_child(&current, name, &walked)?;
            let metadata = current.metadata().map_err(|source| NativeError::Io {
                operation: "inspect destination path component",
                path: walked.clone(),
                source,
            })?;
            if metadata.uid() != 0 || !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
                // /Applications has one documented normal exception handled by its
                // explicit final-component policy in `validate_destination_ancestors`.
                if walked != Path::new("/Applications") {
                    return Err(NativeError::Trust {
                        path: walked,
                        reason: "path component is not a protected root-owned directory",
                    });
                }
            }
        }
    }
    Ok(current)
}

fn open_existing_child(
    parent: &File,
    name: impl AsRef<std::ffi::OsStr>,
    path: &Path,
) -> Result<File, NativeError> {
    let descriptor = openat(
        parent,
        name.as_ref(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| NativeError::Io {
        operation: "open protected directory",
        path: path.to_path_buf(),
        source: io::Error::from(source),
    })?;
    Ok(File::from(descriptor))
}

fn open_or_create_child(
    parent: &File,
    name: &str,
    path: &Path,
    group: Gid,
) -> Result<File, NativeError> {
    open_or_create_child_as(parent, name, path, (Uid::ROOT, group))
}

fn open_or_create_child_as(
    parent: &File,
    name: &str,
    path: &Path,
    owner: (Uid, Gid),
) -> Result<File, NativeError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let descriptor = match openat(parent, name, flags, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => {
            mkdirat(parent, name, mode(ROOT_DIRECTORY_MODE)).map_err(|source| NativeError::Io {
                operation: "create protected directory",
                path: path.to_path_buf(),
                source: io::Error::from(source),
            })?;
            let descriptor =
                openat(parent, name, flags, Mode::empty()).map_err(|source| NativeError::Io {
                    operation: "open newly created protected directory",
                    path: path.to_path_buf(),
                    source: io::Error::from(source),
                })?;
            let directory = File::from(descriptor);
            fchown(&directory, Some(owner.0), Some(owner.1)).map_err(|source| NativeError::Io {
                operation: "set protected directory ownership",
                path: path.to_path_buf(),
                source: io::Error::from(source),
            })?;
            fchmod(&directory, mode(ROOT_DIRECTORY_MODE)).map_err(|source| NativeError::Io {
                operation: "set protected directory mode",
                path: path.to_path_buf(),
                source: io::Error::from(source),
            })?;
            return Ok(directory);
        }
        Err(source) => {
            return Err(NativeError::Io {
                operation: "open protected directory",
                path: path.to_path_buf(),
                source: io::Error::from(source),
            });
        }
    };
    Ok(File::from(descriptor))
}

fn ensure_lease_file(
    directory: &File,
    name: &str,
    uid: u32,
    gid: u32,
) -> Result<File, NativeError> {
    let path = Path::new(LIFECYCLE_DIRECTORY).join(name);
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let (descriptor, created) = match openat(
        directory,
        name,
        flags | OFlags::CREATE | OFlags::EXCL,
        mode(LEASE_FILE_MODE),
    ) {
        Ok(descriptor) => (descriptor, true),
        Err(Errno::EXIST) => (
            openat(directory, name, flags, Mode::empty()).map_err(|source| NativeError::Io {
                operation: "open existing lifecycle lease",
                path: path.clone(),
                source: io::Error::from(source),
            })?,
            false,
        ),
        Err(source) => {
            return Err(NativeError::Io {
                operation: "create lifecycle lease",
                path,
                source: io::Error::from(source),
            });
        }
    };
    let file = File::from(descriptor);
    if created {
        fchown(&file, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid))).map_err(|source| {
            NativeError::Io {
                operation: "set lifecycle lease ownership",
                path: path.clone(),
                source: io::Error::from(source),
            }
        })?;
        fchmod(&file, mode(LEASE_FILE_MODE)).map_err(|source| NativeError::Io {
            operation: "set lifecycle lease mode",
            path: path.clone(),
            source: io::Error::from(source),
        })?;
    }
    inspect_lease_owner(&file, name)?;
    Ok(file)
}

fn inspect_lease_owner(file: &File, name: &str) -> Result<(u32, u32), NativeError> {
    let path = Path::new(LIFECYCLE_DIRECTORY).join(name);
    let metadata = file.metadata().map_err(|source| NativeError::Io {
        operation: "inspect lifecycle lease",
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != u32::from(LEASE_FILE_MODE)
    {
        return Err(NativeError::Trust {
            path,
            reason: "lifecycle lease inode has unexpected links, type or mode",
        });
    }
    Ok((metadata.uid(), metadata.gid()))
}

fn require_removed_distribution() -> Result<(), NativeError> {
    validate_destination_ancestors(Path::new("/"))?;
    require_absent_targets(Path::new("/"))?;
    if launchctl::inspect_registration()? != RegistrationState::Absent {
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
                return Err(NativeError::Io {
                    operation: "inspect distribution before rebinding lifecycle leases",
                    path,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn change_lease_owner(file: &File, owner: (u32, u32)) -> io::Result<()> {
    fchown(
        file,
        Some(Uid::from_raw(owner.0)),
        Some(Gid::from_raw(owner.1)),
    )
    .map_err(io::Error::from)
}

fn inspect_user_lease_owners(leases: &[File; 2]) -> Result<[(u32, u32); 2], NativeError> {
    Ok([
        inspect_lease_owner(&leases[0], USER_LEASE_NAMES[0])?,
        inspect_lease_owner(&leases[1], USER_LEASE_NAMES[1])?,
    ])
}

fn rebind_user_leases(
    leases: &[File; 2],
    previous: &[(u32, u32); 2],
    owner: (u32, u32),
    mut change_owner: impl FnMut(&File, (u32, u32)) -> io::Result<()>,
) -> Result<(), NativeError> {
    for (index, lease) in leases.iter().enumerate() {
        if previous[index] == owner {
            continue;
        }
        if let Err(source) = change_owner(lease, owner) {
            let cause = NativeError::Io {
                operation: "rebind lifecycle lease owner",
                path: Path::new(LIFECYCLE_DIRECTORY).join(USER_LEASE_NAMES[index]),
                source,
            };
            let restore_errors =
                restore_lease_owners(leases, previous, owner, index, &mut change_owner);
            if restore_errors.is_empty() {
                return Err(cause);
            }
            return Err(NativeError::Io {
                operation: "restore lifecycle lease owners after failed account change",
                path: Path::new(LIFECYCLE_DIRECTORY).join(USER_LEASE_NAMES[index]),
                source: io::Error::other(format!(
                    "{cause}; restoration failed: {restore_errors:?}"
                )),
            });
        }
    }
    Ok(())
}

fn restore_lease_owners(
    leases: &[File; 2],
    previous: &[(u32, u32); 2],
    owner: (u32, u32),
    failed_index: usize,
    change_owner: &mut impl FnMut(&File, (u32, u32)) -> io::Result<()>,
) -> Vec<NativeError> {
    let mut errors = Vec::new();
    for index in (0..failed_index).rev() {
        if previous[index] == owner {
            continue;
        }
        if let Err(source) = change_owner(&leases[index], previous[index]) {
            errors.push(NativeError::Io {
                operation: "restore lifecycle lease owner",
                path: Path::new(LIFECYCLE_DIRECTORY).join(USER_LEASE_NAMES[index]),
                source,
            });
        }
    }
    errors
}

fn validate_existing_lease(
    directory: &File,
    name: &str,
    uid: u32,
    gid: u32,
) -> Result<(), NativeError> {
    let path = Path::new(LIFECYCLE_DIRECTORY).join(name);
    let file = openat(
        directory,
        name,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|source| NativeError::Io {
        operation: "open fixed lifecycle lease",
        path: path.clone(),
        source: io::Error::from(source),
    })?;
    if inspect_lease_owner(&file, name)? != (uid, gid) {
        return Err(NativeError::Trust {
            path,
            reason: "fixed lifecycle lease inode has unexpected owner, links, type or mode",
        });
    }
    Ok(())
}

fn validate_root_directory(
    path: &Path,
    directory: &File,
    expected_gid: Option<Gid>,
) -> Result<(), NativeError> {
    let metadata = directory.metadata().map_err(|source| NativeError::Io {
        operation: "inspect lifecycle directory",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir()
        || metadata.uid() != 0
        || expected_gid.is_some_and(|gid| metadata.gid() != gid.as_raw())
        || metadata.mode() & 0o7777 != u32::from(ROOT_DIRECTORY_MODE)
    {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "lifecycle directory must be root:wheel 0755",
        });
    }
    Ok(())
}

fn validate_absolute_normalized(path: &Path) -> Result<(), NativeError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "native path must be absolute and normalized",
        });
    }
    Ok(())
}

fn mode(bits: u16) -> Mode {
    Mode::from_raw_mode(bits)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    };

    use mixrelay_test_support::FixtureDir;

    fn lease_fixture(label: &str) -> (FixtureDir, [File; 2]) {
        let root = FixtureDir::new(label).expect("lease fixture");
        for name in super::USER_LEASE_NAMES
            .into_iter()
            .chain([super::INSTALLER_LEASE_NAME])
        {
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
    fn protected_child_creation_and_existing_link_refusal_are_descriptor_anchored() {
        let root = FixtureDir::new("product-folder-create").expect("fixture");
        let parent = File::open(root.path()).expect("parent descriptor");
        let path = root.join("MixRelay");
        let owner = (rustix::process::geteuid(), rustix::process::getegid());
        let directory = super::open_or_create_child_as(&parent, "MixRelay", &path, owner).unwrap();
        let metadata = directory.metadata().unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.mode() & 0o7777, 0o755);
        assert_eq!(
            (metadata.uid(), metadata.gid()),
            (owner.0.as_raw(), owner.1.as_raw())
        );
        let existing = super::open_or_create_child_as(&parent, "MixRelay", &path, owner).unwrap();
        assert_eq!(existing.metadata().unwrap().ino(), metadata.ino());
        assert_eq!(
            super::validate_root_directory(&path, &directory, None).is_ok(),
            owner.0 == rustix::fs::Uid::ROOT,
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o775)).unwrap();
        assert!(super::validate_root_directory(&path, &directory, None).is_err());
        let outside = root.join("outside");
        fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(super::open_or_create_child_as(&parent, "MixRelay", &path, owner).is_err());
        assert!(outside.is_dir());
        assert_eq!(fs::metadata(outside).unwrap().mode() & 0o7777, 0o775);
    }

    #[test]
    fn cold_reinstall_rebinds_only_user_owners_under_the_existing_guard() {
        let (root, leases) = lease_fixture("lease-rebind");
        let directory = crate::LifecycleDirectory::open(root.path()).expect("directory");
        let guard = directory.acquire().expect("idle owners");
        let previous = super::inspect_user_lease_owners(&leases).expect("safe leases");
        let selected = (previous[0].0 + 1, previous[0].1);
        let metadata = leases
            .each_ref()
            .map(|file| file.metadata().expect("metadata"));
        let mut changes = Vec::new();
        super::rebind_user_leases(&leases, &previous, selected, |file, owner| {
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
                fs::read(root.join(super::USER_LEASE_NAMES[index])).expect("content"),
                super::USER_LEASE_NAMES[index].as_bytes()
            );
        }
        assert_eq!(
            fs::read(root.join(super::INSTALLER_LEASE_NAME)).expect("installer content"),
            super::INSTALLER_LEASE_NAME.as_bytes()
        );
        drop(guard);
        drop(directory.acquire().expect("all locks released"));
    }

    #[test]
    fn same_account_keeps_existing_owners_without_a_change_call() {
        let (_root, leases) = lease_fixture("lease-same-account");
        let previous = super::inspect_user_lease_owners(&leases).expect("owners");
        super::rebind_user_leases(&leases, &previous, previous[0], |_, _| {
            panic!("unchanged ownership must not invoke fchown")
        })
        .expect("same account");
    }

    #[test]
    fn every_active_owner_prevents_the_rebinding_guard() {
        for name in super::USER_LEASE_NAMES
            .into_iter()
            .chain([super::INSTALLER_LEASE_NAME])
        {
            let (root, leases) = lease_fixture("lease-rebind-busy");
            let owner = File::options()
                .read(true)
                .write(true)
                .open(root.join(name))
                .expect("active owner descriptor");
            owner.try_lock().expect("active owner");
            let before = super::inspect_user_lease_owners(&leases).expect("owners");
            let directory = crate::LifecycleDirectory::open(root.path()).expect("directory");
            assert!(matches!(
                directory.acquire(),
                Err(crate::LeaseError::Busy { .. })
            ));
            assert_eq!(
                super::inspect_user_lease_owners(&leases).expect("retained owners"),
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
        for target in crate::OwnedTarget::all() {
            let root = FixtureDir::new("lease-retained-installation").expect("fixture");
            super::require_absent_targets(&root).expect("fully removed");
            let path = root.join(target.path().trim_start_matches('/'));
            fs::create_dir_all(path.parent().expect("target parent")).expect("parent");
            fs::write(&path, b"installed").expect("retained target");
            assert!(matches!(super::require_absent_targets(&root),
                Err(super::NativeError::Trust { path: rejected, .. }) if rejected == path));
        }
    }

    #[test]
    fn unsafe_existing_leases_are_rejected_before_account_changes() {
        let (root, leases) = lease_fixture("lease-unsafe-mode");
        fs::set_permissions(root.join("app.owner"), fs::Permissions::from_mode(0o600))
            .expect("wrong mode");
        assert!(super::inspect_user_lease_owners(&leases).is_err());
        fs::set_permissions(root.join("app.owner"), fs::Permissions::from_mode(0o644))
            .expect("restore mode");
        fs::hard_link(root.join("app.owner"), root.join("alias")).expect("second link");
        assert!(super::inspect_user_lease_owners(&leases).is_err());
        let (root, _leases) = lease_fixture("lease-unsafe-symlink");
        fs::remove_file(root.join("app.owner")).expect("fixture path");
        symlink("engine.owner", root.join("app.owner")).expect("symlink");
        let directory = crate::LifecycleDirectory::open(root.path()).expect("directory");
        assert!(directory.acquire().is_err());
    }

    #[test]
    fn partial_owner_change_failure_restores_earlier_owners() {
        let (_root, leases) = lease_fixture("lease-rebind-restore");
        let previous = super::inspect_user_lease_owners(&leases).expect("owners");
        let selected = (previous[0].0 + 1, previous[0].1);
        let mut changes = Vec::new();
        let result = super::rebind_user_leases(&leases, &previous, selected, |_, owner| {
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
        let previous = super::inspect_user_lease_owners(&leases).expect("owners");
        let selected = (previous[0].0 + 1, previous[0].1);
        let mut calls = 0;
        let error = super::rebind_user_leases(&leases, &previous, selected, |_, _| {
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
        for name in super::USER_LEASE_NAMES {
            assert!(
                detail.contains(name),
                "missing failure path {name}: {detail}"
            );
        }
        assert!(detail.contains("restoration failed"));
    }

    #[test]
    fn privileged_operations_require_root_with_the_wheel_effective_group() {
        assert!(super::root_wheel_identity(0, 0, 0));
        assert!(!super::root_wheel_identity(0, 20, 0));
        assert!(!super::root_wheel_identity(501, 0, 0));
    }

    #[test]
    fn applications_group_writable_exception_is_pinned_to_exact_policy() {
        // This fixture is intentionally a pure policy test: it does not chmod or
        // inspect the host's /Applications directory.
        assert_eq!(super::ROOT_DIRECTORY_MODE, 0o755);
        assert_eq!(super::LEASE_FILE_MODE, 0o644);
        assert_eq!(
            super::APP_DIRECTORY,
            "/Library/Application Support/MixRelay"
        );
        assert!(super::applications_directory_policy(0, 80, 0o775, 80));
        assert!(super::applications_directory_policy(0, 80, 0o755, 80));
        assert!(!super::applications_directory_policy(501, 80, 0o775, 80));
        assert!(!super::applications_directory_policy(0, 80, 0o777, 80));
    }
}
