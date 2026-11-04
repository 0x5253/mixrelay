//! Ownership, type and mode rules for the fixed native paths, decided from inode facts.
//!
//! Each rule returns the reason it rejected an inode; adapters attach the path.

use std::{fs::Metadata, os::unix::fs::MetadataExt, path::Path};

pub(super) const ROOT_DIRECTORY_MODE: u16 = 0o755;
pub(super) const LEASE_FILE_MODE: u16 = 0o644;
pub(super) const MAX_LAUNCHD_PLIST_BYTES: u64 = 16 * 1024;
const LAUNCHD_PLIST_MODE: u32 = 0o644;
/// macOS ships `/Applications` as root:admin with this mode.
const APPLICATIONS_ADMIN_MODE: u32 = 0o775;

/// Rejection reason for one inode.
pub(super) type Decision = Result<(), &'static str>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InodeKind {
    Directory,
    File,
    Symlink,
    Other,
}

/// The facts a trust rule may read from one inode; `mode` holds only permission bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Inode {
    pub(super) kind: InodeKind,
    pub(super) uid: u32,
    pub(super) gid: u32,
    pub(super) mode: u32,
    pub(super) links: u64,
    pub(super) len: u64,
}

impl Inode {
    pub(super) fn of(metadata: &Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            InodeKind::Symlink
        } else if file_type.is_dir() {
            InodeKind::Directory
        } else if file_type.is_file() {
            InodeKind::File
        } else {
            InodeKind::Other
        };
        Self {
            kind,
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode() & 0o7777,
            links: metadata.nlink(),
            len: metadata.len(),
        }
    }

    fn is_root_directory(self) -> bool {
        self.kind == InodeKind::Directory && self.uid == 0
    }

    fn writable_by_group_or_other(self) -> bool {
        self.mode & 0o022 != 0
    }

    fn is_single_link_file(self) -> bool {
        self.kind == InodeKind::File && self.links == 1
    }
}

/// Where a fixed destination parent sits, which selects its mode rule.
#[derive(Clone, Copy, Debug)]
pub(super) enum ParentRule {
    /// `/Applications`, whose normal macOS mode is group-writable by admin.
    Applications { admin_gid: u32 },
    /// Every other parent, which nobody but root may write.
    Protected,
}

/// Each component walked toward a fixed path. `/Applications` is checked by its own rule.
pub(super) fn path_component(path: &Path, inode: Inode) -> Decision {
    if path == Path::new("/Applications")
        || (inode.is_root_directory() && !inode.writable_by_group_or_other())
    {
        Ok(())
    } else {
        Err("path component is not a protected root-owned directory")
    }
}

/// Accepts macOS's root:admin 0775 `/Applications` or any root-owned mode closed to
/// group and other writes; nothing else is rewritten.
pub(super) fn applications_directory(inode: Inode, admin_gid: u32) -> Decision {
    let admin_default = inode.gid == admin_gid && inode.mode == APPLICATIONS_ADMIN_MODE;
    if inode.uid == 0 && (admin_default || !inode.writable_by_group_or_other()) {
        Ok(())
    } else {
        Err("Applications must retain root:admin 0775 or a restrictive root-owned mode")
    }
}

/// An existing parent of an install target.
pub(super) fn destination_parent(inode: Inode, rule: ParentRule) -> Decision {
    if !inode.is_root_directory() {
        return Err("destination parent must be a root-owned directory");
    }
    match rule {
        ParentRule::Applications { admin_gid } => applications_directory(inode, admin_gid),
        ParentRule::Protected if inode.writable_by_group_or_other() => {
            Err("destination parent must not be group/other writable")
        }
        ParentRule::Protected => Ok(()),
    }
}

/// A directory the installer creates or owns: root, the expected group, exactly 0755.
pub(super) fn root_directory(inode: Inode, expected_gid: Option<u32>) -> Decision {
    if inode.is_root_directory()
        && expected_gid.is_none_or(|gid| inode.gid == gid)
        && inode.mode == u32::from(ROOT_DIRECTORY_MODE)
    {
        Ok(())
    } else {
        Err("lifecycle directory must be root:wheel 0755")
    }
}

/// The engine's parent, read without following a link.
pub(super) fn engine_directory(inode: Inode, wheel_gid: u32) -> Decision {
    root_directory(inode, Some(wheel_gid))
        .map_err(|_| "engine parent must be a direct root:wheel 0755 directory")
}

/// An installed target owned by root:wheel (gid 0) with its fixed distribution mode.
pub(super) fn owned_target(inode: Inode, mode: u32) -> Decision {
    if inode.kind == InodeKind::Symlink || inode.uid != 0 || inode.gid != 0 {
        return Err("owned target must be a direct root:wheel inode");
    }
    if inode.mode != mode {
        return Err("owned target mode differs from the fixed distribution policy");
    }
    Ok(())
}

/// A lifecycle lease: one link to a regular 0644 file. Its owner is checked separately.
pub(super) fn lease(inode: Inode) -> Decision {
    if inode.is_single_link_file() && inode.mode == u32::from(LEASE_FILE_MODE) {
        Ok(())
    } else {
        Err("lifecycle lease inode has unexpected links, type or mode")
    }
}

/// A user lease kept after the plist is gone. Root never counts as the service account.
pub(super) fn retained_account_lease(inode: Inode) -> Decision {
    if lease(inode).is_ok() && inode.uid != 0 {
        Ok(())
    } else {
        Err("invalid retained service account lease")
    }
}

/// The installed Driver plist, small enough to read in full.
pub(super) fn installed_launchd_plist(inode: Inode, wheel_gid: u32) -> Decision {
    if inode.is_single_link_file()
        && inode.uid == 0
        && inode.gid == wheel_gid
        && inode.mode == LAUNCHD_PLIST_MODE
        && inode.len <= MAX_LAUNCHD_PLIST_BYTES
    {
        Ok(())
    } else {
        Err("installed launchd plist must be a bounded root:wheel 0644 regular inode")
    }
}

#[cfg(test)]
mod tests;
