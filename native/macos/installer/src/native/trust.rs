//! Ownership and mode checks for the fixed native install paths.
//!
//! `policy` decides from inode facts. Directory walks and the installed plist read
//! open each level with `O_NOFOLLOW` and create only missing fixed levels. Single-path
//! checks, such as the engine directory and owned targets, read `symlink_metadata` by
//! path: a link is reported as itself and rejected, but the check is not anchored to
//! an opened parent descriptor.

mod anchored;
pub(super) mod destination;
pub(super) mod installed_plist;
pub(super) mod lifecycle;
mod policy;
mod product;

use std::path::Path;

use rustix::fs::Gid;

use super::{NativeError, account::group_id_for_name};

const APP_DIRECTORY: &str = "/Library/Application Support/MixRelay";
const LAUNCH_DAEMONS_DIRECTORY: &str = "/Library/LaunchDaemons";

/// Attaches the path to a rejected trust decision.
fn require(path: &Path, decision: policy::Decision) -> Result<(), NativeError> {
    decision.map_err(|reason| NativeError::Trust {
        path: path.to_path_buf(),
        reason,
    })
}

fn wheel_group() -> Result<Gid, NativeError> {
    group_id_for_name("wheel").map(Gid::from_raw)
}

fn admin_gid() -> Result<u32, NativeError> {
    group_id_for_name("admin")
}
