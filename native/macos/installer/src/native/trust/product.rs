//! Configure a new product inode privately before exclusive publication.

use std::{ffi::OsStr, fs::File, io};

use rustix::fs::{AtFlags, Gid, Mode, Uid, fchmod, fchown, mkdirat, unlinkat};

use crate::directory::{create_private_child, open_child, rename_exclusive};

pub(super) fn create_product_directory_at(
    applications: &File,
    lifecycle: &File,
    owner: (Uid, Gid),
    before_publish: impl FnOnce(),
) -> io::Result<File> {
    let name = OsStr::new("MixRelay");
    match open_child(applications, name) {
        Ok(existing) => return Ok(existing),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let staging_name = OsStr::new(".product-folder-create");
    let staging = create_private_child(lifecycle, staging_name)?;
    let result = prepare_and_publish(applications, &staging, owner, before_publish);
    unlinkat(lifecycle, staging_name, AtFlags::REMOVEDIR)?;
    result
}

fn prepare_and_publish(
    applications: &File,
    staging: &File,
    owner: (Uid, Gid),
    before_publish: impl FnOnce(),
) -> io::Result<File> {
    let name = OsStr::new("MixRelay");
    mkdirat(staging, name, Mode::RWXU)?;
    let result = (|| {
        let directory = open_child(staging, name)?;
        fchown(&directory, Some(owner.0), Some(owner.1))?;
        fchmod(&directory, Mode::from_raw_mode(0o755))?;
        before_publish();
        match rename_exclusive(staging, name, applications, name) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        // The caller validates this competing inode without modifying it.
        open_child(applications, name)
    })();
    match unlinkat(staging, name, AtFlags::REMOVEDIR) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => result,
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
