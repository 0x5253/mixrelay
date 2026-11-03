//! Console identity resolution through the system passwd and group databases.

use std::{
    ffi::{CStr, CString},
    fs,
    mem::MaybeUninit,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Component, Path, PathBuf},
};

use crate::ServiceAccount;

use super::NativeError;

const MAX_ACCOUNT_BUFFER: usize = 1024 * 1024;
const DEFAULT_ACCOUNT_BUFFER: usize = 16 * 1024;

/// A service identity resolved from the active console user's OS records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedServiceAccount {
    account: ServiceAccount,
    uid: u32,
    gid: u32,
}

impl ResolvedServiceAccount {
    /// Returns the validated account names and home path bound into launchd.
    #[must_use]
    pub fn account(&self) -> &ServiceAccount {
        &self.account
    }

    /// Returns the numeric UID returned by the system passwd database.
    #[must_use]
    pub const fn uid(&self) -> u32 {
        self.uid
    }

    /// Returns the numeric primary GID returned by the system group database.
    #[must_use]
    pub const fn gid(&self) -> u32 {
        self.gid
    }
}

/// Resolves `/dev/console` to a real non-root passwd/group record and validates its home.
///
/// No environment variables, shell output, display name, or caller-supplied account
/// string participates in the identity choice. The resulting UID, primary GID and
/// home are later re-resolved under installer serialization before mutation.
/// # Errors
/// Returns an error when there is no logged-in console user or any OS record/path
/// fails validation.
pub fn resolve_console_account() -> Result<ResolvedServiceAccount, NativeError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let console_path = Path::new("/dev/console");
    let link_metadata = fs::symlink_metadata(console_path).map_err(|error| NativeError::Io {
        operation: "inspect console device",
        path: console_path.to_path_buf(),
        source: error,
    })?;
    if link_metadata.file_type().is_symlink() || !link_metadata.file_type().is_char_device() {
        return Err(NativeError::Account(
            "/dev/console is not a direct console device".to_owned(),
        ));
    }
    let uid = link_metadata.uid();
    if uid == 0 {
        return Err(NativeError::Account(
            "the console device is not owned by a logged-in non-root user".to_owned(),
        ));
    }

    resolve_service_uid(uid)
}

/// Resolves the retained lifecycle lease owner even when the Driver plist is gone.
pub(super) fn resolve_service_uid(uid: u32) -> Result<ResolvedServiceAccount, NativeError> {
    let record = passwd_for_uid(uid)?;
    if record.uid != uid || record.name == "root" {
        return Err(NativeError::Account(
            "service UID does not match a non-root passwd record".to_owned(),
        ));
    }
    let group_name = group_name_for_gid(record.gid)?;
    let account = ServiceAccount::new(record.name, group_name, record.home)
        .map_err(|error| NativeError::Account(error.to_string()))?;
    let resolved = resolve_service_account(&account)?;
    if resolved.uid != uid {
        return Err(NativeError::Account(
            "service UID changed during account resolution".to_owned(),
        ));
    }
    Ok(resolved)
}

/// Re-resolves one previously selected service identity through passwd/group and
/// validates its numeric IDs and home ownership without depending on who is
/// currently logged in at the console. This is required for safe upgrades and
/// removal when a different administrator is logged in.
pub(super) fn resolve_service_account(
    account: &ServiceAccount,
) -> Result<ResolvedServiceAccount, NativeError> {
    let record = passwd_for_name(account.user())?;
    if record.name != account.user() || record.uid == 0 || record.gid == 0 {
        return Err(NativeError::Account(
            "service identity does not resolve to a non-root passwd record".to_owned(),
        ));
    }
    let group_name = group_name_for_gid(record.gid)?;
    if group_name != account.group() || record.home.as_path() != account.home() {
        return Err(NativeError::Account(
            "service identity differs from its passwd primary group or home".to_owned(),
        ));
    }
    validate_home(account, record.uid, record.gid)?;
    Ok(ResolvedServiceAccount {
        account: account.clone(),
        uid: record.uid,
        gid: record.gid,
    })
}

pub(super) fn group_id_for_name(name: &str) -> Result<u32, NativeError> {
    let c_name = CString::new(name)
        .map_err(|_| NativeError::Account("group name contains NUL".to_owned()))?;
    let mut buffer = account_buffer();
    let mut entry = MaybeUninit::<libc::group>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: the buffer and result pointer stay live through the reentrant lookup;
    // the returned group record is copied into owned Rust values before the buffer drops.
    let status = unsafe {
        libc::getgrnam_r(
            c_name.as_ptr(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(NativeError::Account(format!(
            "system group database has no usable {name:?} group (status {status})"
        )));
    }
    // SAFETY: a non-null result from successful getgrnam_r initializes `entry`.
    let entry = unsafe { entry.assume_init() };
    if entry.gr_name.is_null() {
        return Err(NativeError::Account(
            "group record is missing its name".to_owned(),
        ));
    }
    // SAFETY: POSIX guarantees this string is NUL-terminated in the live buffer.
    let resolved_name = unsafe { CStr::from_ptr(entry.gr_name) }
        .to_str()
        .map_err(|_| NativeError::Account("group name is not UTF-8".to_owned()))?;
    if resolved_name != name {
        return Err(NativeError::Account(
            "system group lookup returned a different group name".to_owned(),
        ));
    }
    Ok(entry.gr_gid)
}

fn passwd_for_uid(uid: u32) -> Result<PasswdRecord, NativeError> {
    let mut buffer = account_buffer();
    let mut entry = MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: all out-pointers and the scratch buffer are valid for this reentrant call.
    let status = unsafe {
        libc::getpwuid_r(
            uid as libc::uid_t,
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(NativeError::Account(format!(
            "system passwd database has no usable UID {uid} record (status {status})"
        )));
    }
    // SAFETY: a non-null result from successful getpwuid_r initializes `entry`.
    let entry = unsafe { entry.assume_init() };
    if entry.pw_name.is_null() || entry.pw_dir.is_null() {
        return Err(NativeError::Account(
            "passwd record is missing its name or home".to_owned(),
        ));
    }
    // SAFETY: POSIX guarantees these strings are NUL-terminated in the live buffer.
    let name = unsafe { CStr::from_ptr(entry.pw_name) }
        .to_str()
        .map_err(|_| NativeError::Account("passwd name is not UTF-8".to_owned()))?
        .to_owned();
    // SAFETY: same lifetime and NUL-termination guarantee as `pw_name` above.
    let home = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes().to_vec();
    let home = PathBuf::from(std::ffi::OsString::from_vec(home));
    Ok(PasswdRecord {
        name,
        uid: entry.pw_uid,
        gid: entry.pw_gid,
        home,
    })
}

fn passwd_for_name(name: &str) -> Result<PasswdRecord, NativeError> {
    let c_name = CString::new(name)
        .map_err(|_| NativeError::Account("account name contains NUL".to_owned()))?;
    let mut buffer = account_buffer();
    let mut entry = MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: all out-pointers and the scratch buffer are valid for this reentrant call.
    let status = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(NativeError::Account(format!(
            "system passwd database has no usable account {name:?} record (status {status})"
        )));
    }
    // SAFETY: a non-null result from successful getpwnam_r initializes `entry`.
    let entry = unsafe { entry.assume_init() };
    if entry.pw_name.is_null() || entry.pw_dir.is_null() {
        return Err(NativeError::Account(
            "passwd record is missing its name or home".to_owned(),
        ));
    }
    // SAFETY: POSIX guarantees these strings are NUL-terminated in the live buffer.
    let name = unsafe { CStr::from_ptr(entry.pw_name) }
        .to_str()
        .map_err(|_| NativeError::Account("passwd name is not UTF-8".to_owned()))?
        .to_owned();
    // SAFETY: same lifetime and NUL-termination guarantee as `pw_name` above.
    let home = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes().to_vec();
    let home = PathBuf::from(std::ffi::OsString::from_vec(home));
    Ok(PasswdRecord {
        name,
        uid: entry.pw_uid,
        gid: entry.pw_gid,
        home,
    })
}

fn group_name_for_gid(gid: u32) -> Result<String, NativeError> {
    let mut buffer = account_buffer();
    let mut entry = MaybeUninit::<libc::group>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: all out-pointers and the scratch buffer are valid for this reentrant call.
    let status = unsafe {
        libc::getgrgid_r(
            gid as libc::gid_t,
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(NativeError::Account(format!(
            "system group database has no usable GID {gid} record (status {status})"
        )));
    }
    // SAFETY: a non-null result from successful getgrgid_r initializes `entry`.
    let entry = unsafe { entry.assume_init() };
    if entry.gr_name.is_null() || entry.gr_gid != gid as libc::gid_t {
        return Err(NativeError::Account(
            "primary group lookup returned a different GID or missing name".to_owned(),
        ));
    }
    // SAFETY: POSIX guarantees this string is NUL-terminated in the live buffer.
    unsafe { CStr::from_ptr(entry.gr_name) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| NativeError::Account("primary group name is not UTF-8".to_owned()))
}

fn account_buffer() -> Vec<u8> {
    // POSIX permits sysconf to return -1 when no limit is known. The fixed fallback
    // is intentionally bounded and lookup errors remain hard failures.
    // SAFETY: sysconf is a read-only process query with no pointer arguments.
    let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let size = usize::try_from(suggested)
        .ok()
        .filter(|size| *size > 0 && *size <= MAX_ACCOUNT_BUFFER)
        .unwrap_or(DEFAULT_ACCOUNT_BUFFER);
    vec![0; size]
}

fn validate_home(account: &ServiceAccount, uid: u32, gid: u32) -> Result<(), NativeError> {
    use std::os::unix::fs::MetadataExt;

    let home = account.home();
    if !home.is_absolute()
        || home
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(NativeError::Account(
            "passwd home is not an absolute normalized path".to_owned(),
        ));
    }
    let mut current = PathBuf::from("/");
    for component in home.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| NativeError::Io {
            operation: "inspect account home component",
            path: current.clone(),
            source: error,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != 0 {
            // The final home directory is user-owned; parent components are root-owned.
            let is_final = current == home;
            if !(is_final && metadata.is_dir() && !metadata.file_type().is_symlink()) {
                return Err(NativeError::Account(format!(
                    "home path component {} is not a trusted directory",
                    current.display()
                )));
            }
        }
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(NativeError::Account(format!(
                "home path component {} is group/other writable",
                current.display()
            )));
        }
    }
    let metadata = fs::symlink_metadata(home).map_err(|error| NativeError::Io {
        operation: "inspect account home",
        path: home.to_path_buf(),
        source: error,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != uid {
        return Err(NativeError::Account(
            "home directory is not owned by the resolved console UID".to_owned(),
        ));
    }
    if metadata.gid() != gid {
        return Err(NativeError::Account(
            "home directory group differs from the passwd primary GID".to_owned(),
        ));
    }
    Ok(())
}

struct PasswdRecord {
    name: String,
    uid: u32,
    gid: u32,
    home: PathBuf,
}
