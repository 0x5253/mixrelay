use std::{
    fs,
    fs::File,
    io,
    path::{Component, Path},
};

use crate::StateError;

pub(super) struct StoreDirectory {
    file: File,
}

impl StoreDirectory {
    pub(super) fn open(path: &Path, create: bool) -> Result<Self, StateError> {
        use std::os::unix::fs::MetadataExt;

        use rustix::fs::{Mode, OFlags, fchmod, mkdirat, open, openat};

        if !path.is_absolute() {
            return Err(StateError::UnsafeStoreDirectory);
        }
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => names.push(name),
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                    return Err(StateError::UnsafeStoreDirectory);
                }
            }
        }
        let Some((last, parents)) = names.split_last() else {
            return Err(StateError::UnsafeStoreDirectory);
        };

        let mut current = File::from(
            open(
                "/",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        for component in parents {
            current = File::from(
                openat(
                    &current,
                    *component,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .map_err(io::Error::from)?,
            );
        }

        let created = if create {
            match mkdirat(&current, *last, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
                Ok(()) => true,
                Err(rustix::io::Errno::EXIST) => false,
                Err(error) => return Err(StateError::Io(io::Error::from(error))),
            }
        } else {
            false
        };
        let directory = File::from(
            openat(
                &current,
                *last,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(StateError::UnsafeStoreDirectory);
        }
        if created {
            fchmod(&directory, Mode::RUSR | Mode::WUSR | Mode::XUSR).map_err(io::Error::from)?;
        } else if metadata.mode() & 0o7777 != 0o700 {
            return Err(StateError::UnsafeStoreDirectory);
        }
        Ok(Self { file: directory })
    }

    pub(super) fn open_file(&self, name: &str) -> io::Result<File> {
        use rustix::fs::{Mode, OFlags, openat};

        let descriptor = openat(
            &self.file,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        Ok(File::from(descriptor))
    }

    pub(super) fn owns_blob(metadata: &fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;

        metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600
            && metadata.nlink() == 1
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    pub(super) fn create_temp(&self, name: &str) -> io::Result<File> {
        use rustix::fs::{AtFlags, Mode, OFlags, fchmod, openat, unlinkat};

        let descriptor = openat(
            &self.file,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io::Error::from)?;
        let file = File::from(descriptor);
        if let Err(error) = fchmod(&file, Mode::RUSR | Mode::WUSR) {
            let _ = unlinkat(&self.file, name, AtFlags::empty());
            return Err(io::Error::from(error));
        }
        Ok(file)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn publish_no_replace(&self, from: &str, to: &str) -> io::Result<()> {
        use rustix::fs::{RenameFlags, renameat_with};

        renameat_with(&self.file, from, &self.file, to, RenameFlags::NOREPLACE)
            .map_err(io::Error::from)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn publish_no_replace(&self, from: &str, to: &str) -> io::Result<()> {
        use rustix::fs::{AtFlags, linkat};

        linkat(&self.file, from, &self.file, to, AtFlags::empty()).map_err(io::Error::from)
    }

    pub(super) fn remove_file(&self, name: &str) {
        use rustix::fs::{AtFlags, unlinkat};

        let _ = unlinkat(&self.file, name, AtFlags::empty());
    }
}
