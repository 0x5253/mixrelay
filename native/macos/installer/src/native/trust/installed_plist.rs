//! Reads the installed Driver plist that names the existing installation's account.

use std::{fs::File, io::Read, path::Path};

use rustix::{
    fs::{Mode, OFlags, openat},
    io::Errno,
};

use super::{
    LAUNCH_DAEMONS_DIRECTORY,
    anchored::{errno_error, inspect, io_error, open_protected_directory},
    policy::{self, MAX_LAUNCHD_PLIST_BYTES},
    require, wheel_group,
};
use crate::{LAUNCHD_PATH, native::NativeError};

const PLIST_NAME: &str = "io.mixrelay.audio.driver.plist";

/// Reads the fixed installed launchd plist through a trusted directory descriptor.
/// Missing is reported only for an actual `ENOENT`; malformed or unsafe objects
/// remain errors so callers never fall back to a different console identity.
pub(in crate::native) fn read_installed_launchd_plist() -> Result<Option<Vec<u8>>, NativeError> {
    let path = Path::new(LAUNCHD_PATH);
    let parent = open_protected_directory(Path::new(LAUNCH_DAEMONS_DIRECTORY))?;
    let wheel = wheel_group()?;
    let file = match openat(
        &parent,
        PLIST_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => File::from(descriptor),
        Err(Errno::NOENT) => return Ok(None),
        Err(source) => return Err(errno_error("open installed launchd plist", path, source)),
    };
    let inode = inspect(&file, "inspect installed launchd plist", path)?;
    require(path, policy::installed_launchd_plist(inode, wheel.as_raw()))?;
    read_bounded(file, inode.len, path).map(Some)
}

/// Reads at most one byte past the limit so a file that grew after inspection is rejected.
fn read_bounded(file: File, expected_len: u64, path: &Path) -> Result<Vec<u8>, NativeError> {
    let capacity = usize::try_from(expected_len).map_err(|_| NativeError::Trust {
        path: path.to_path_buf(),
        reason: "installed launchd plist size exceeds the supported address space",
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_LAUNCHD_PLIST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| io_error("read installed launchd plist", path, source))?;
    if bytes.len() as u64 > MAX_LAUNCHD_PLIST_BYTES {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "installed launchd plist exceeds its fixed size limit",
        });
    }
    Ok(bytes)
}
