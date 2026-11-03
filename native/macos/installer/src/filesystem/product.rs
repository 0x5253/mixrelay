//! Product-folder identity and empty-only cleanup; app bundles remain separate targets.

use std::{
    ffi::OsStr,
    fs::{self, File},
    io,
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

use rustix::fs::{AtFlags, unlinkat};

use crate::directory::{create_private_child, open_child, open_directory, rename_exclusive};

use super::{
    FilesystemAdapter, FilesystemError, FilesystemIdentity, FilesystemSnapshot, MutationProgress,
    PRODUCT_DIRECTORY, identity, io_failure,
};

#[derive(Clone, Copy)]
enum ProductMove {
    ToPrivate,
    FromPrivate,
}

impl FilesystemAdapter {
    pub(super) fn rollback_installed_product(
        &self,
        product: Option<&File>,
        staged: &super::FilesystemStaged,
        installed: &super::InstalledTarget,
    ) -> Result<(), FilesystemError> {
        let destination = self.layout.target_path(installed.target);
        let directory = product.ok_or_else(|| FilesystemError::CreatedDirectoryChanged {
            path: self.product_directory_path(),
        })?;
        let name = destination
            .file_name()
            .ok_or_else(|| FilesystemError::UnsafePath {
                path: destination.clone(),
                reason: "target has no name",
            })?;
        let file = open_child(directory, name)
            .map_err(|error| io_failure("open installed product", &destination, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_failure("inspect installed product", &destination, error))?;
        if super::fingerprint(&metadata) != installed.fingerprint {
            return Err(FilesystemError::ConcurrentTargetChange {
                target: installed.target,
            });
        }
        let payload = staged.payload_path(installed.target);
        self.move_target_to_private(product, installed.target, &payload)?;
        super::remove_installed_path(&payload, installed)
    }

    pub(super) fn product_directory_path(&self) -> PathBuf {
        self.layout
            .root()
            .join(PRODUCT_DIRECTORY.trim_start_matches('/'))
    }

    pub(super) fn observe_product_directory(
        &self,
    ) -> Result<Option<FilesystemIdentity>, FilesystemError> {
        let path = self.product_directory_path();
        super::ensure_no_symlink_components(&path, true)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_failure("inspect product folder", &path, error)),
        };
        // Fixtures use their normal owner; production requires root.
        let owner = if self.layout.root() == std::path::Path::new("/") {
            0
        } else {
            rustix::process::geteuid().as_raw()
        };
        if !metadata.is_dir() || !product_directory_policy(metadata.uid(), metadata.mode(), owner) {
            return Err(FilesystemError::UnsafePath {
                path,
                reason: "product folder must be a protected owned directory",
            });
        }
        Ok(Some(identity(&metadata)))
    }

    pub(super) fn open_product_directory(
        &self,
        snapshot: &FilesystemSnapshot,
        progress: &MutationProgress,
    ) -> Result<Option<File>, FilesystemError> {
        let path = self.product_directory_path();
        let expected = snapshot.product_directory.or_else(|| {
            progress
                .created_directories()
                .iter()
                .find(|directory| directory.path == path)
                .map(|directory| directory.identity)
        });
        let Some(expected) = expected else {
            if self.observe_product_directory()?.is_some() {
                return Err(FilesystemError::CreatedDirectoryChanged { path });
            }
            return Ok(None);
        };
        let parent = open_directory(path.parent().ok_or_else(|| FilesystemError::UnsafePath {
            path: path.clone(),
            reason: "product folder has no parent",
        })?)
        .map_err(|error| io_failure("open Applications", &path, error))?;
        let directory = open_child(&parent, OsStr::new("MixRelay"))
            .map_err(|error| io_failure("open product folder", &path, error))?;
        let metadata = directory
            .metadata()
            .map_err(|error| io_failure("inspect product descriptor", &path, error))?;
        let owner = if self.layout.root() == std::path::Path::new("/") {
            0
        } else {
            rustix::process::geteuid().as_raw()
        };
        if identity(&metadata) != expected
            || !product_directory_policy(metadata.uid(), metadata.mode(), owner)
        {
            return Err(FilesystemError::CreatedDirectoryChanged { path });
        }
        Ok(Some(directory))
    }

    pub(super) fn remove_empty_product_directory(
        &self,
        expected: Option<FilesystemIdentity>,
    ) -> Result<(), FilesystemError> {
        self.remove_empty_product_directory_with(expected, || {})
    }

    fn remove_empty_product_directory_with(
        &self,
        expected: Option<FilesystemIdentity>,
        before_capture: impl FnOnce(),
    ) -> Result<(), FilesystemError> {
        let observed = self.observe_product_directory()?;
        if observed.is_none() {
            return Ok(());
        }
        if observed != expected {
            return Err(FilesystemError::CreatedDirectoryChanged {
                path: self.product_directory_path(),
            });
        }
        let path = self.product_directory_path();
        let applications =
            open_directory(path.parent().ok_or_else(|| FilesystemError::UnsafePath {
                path: path.clone(),
                reason: "product folder has no parent",
            })?)
            .map_err(|error| io_failure("open Applications", &path, error))?;
        // A sibling of transaction staging survives mandatory staging cleanup.
        let lifecycle_path =
            self.layout
                .staging_root()
                .parent()
                .ok_or_else(|| FilesystemError::UnsafePath {
                    path: path.clone(),
                    reason: "staging has no lifecycle parent",
                })?;
        let lifecycle = open_directory(lifecycle_path)
            .map_err(|error| io_failure("open lifecycle", lifecycle_path, error))?;
        let holding_name = OsStr::new(".product-folder-holding");
        let holding_path = lifecycle_path.join(holding_name);
        let holding = create_private_child(&lifecycle, holding_name).map_err(|error| {
            io_failure("create product holding directory", &holding_path, error)
        })?;
        before_capture();
        let result = capture_empty_product(&applications, &holding, expected, &path, || {});
        // rmdir preserves any captured entry that could not be restored.
        unlinkat(&lifecycle, holding_name, AtFlags::REMOVEDIR).map_err(|error| {
            io_failure(
                "remove product holding directory",
                &holding_path,
                error.into(),
            )
        })?;
        result
    }
}

fn capture_empty_product(
    applications: &File,
    holding: &File,
    expected: Option<FilesystemIdentity>,
    path: &std::path::Path,
    after_capture: impl FnOnce(),
) -> Result<(), FilesystemError> {
    let name = OsStr::new("MixRelay");
    rename_exclusive(applications, name, holding, name)
        .map_err(|error| io_failure("capture product folder", path, error))?;
    after_capture();
    let captured = open_child(holding, name).and_then(|directory| directory.metadata());
    let result = match captured {
        Ok(metadata) if Some(identity(&metadata)) == expected => {
            match unlinkat(holding, name, AtFlags::REMOVEDIR) {
                Ok(()) => return Ok(()),
                Err(rustix::io::Errno::NOTEMPTY) => Ok(()),
                Err(error) => Err(io_failure(
                    "remove captured product folder",
                    path,
                    error.into(),
                )),
            }
        }
        Ok(_) => Err(FilesystemError::CreatedDirectoryChanged {
            path: path.to_path_buf(),
        }),
        Err(error) => Err(io_failure("inspect captured product folder", path, error)),
    };
    rename_exclusive(holding, name, applications, name)
        .map_err(|error| io_failure("restore captured product folder", path, error))?;
    result
}

impl FilesystemAdapter {
    pub(super) fn move_target_to_private(
        &self,
        product: Option<&File>,
        target: crate::OwnedTarget,
        private: &std::path::Path,
    ) -> Result<(), FilesystemError> {
        self.move_product_target(product, target, private, ProductMove::ToPrivate)
    }

    pub(super) fn move_private_to_target(
        &self,
        product: Option<&File>,
        target: crate::OwnedTarget,
        private: &std::path::Path,
    ) -> Result<(), FilesystemError> {
        self.move_product_target(product, target, private, ProductMove::FromPrivate)
    }

    fn move_product_target(
        &self,
        product: Option<&File>,
        target: crate::OwnedTarget,
        private: &std::path::Path,
        direction: ProductMove,
    ) -> Result<(), FilesystemError> {
        let destination = self.layout.target_path(target);
        if !matches!(
            target,
            crate::OwnedTarget::App | crate::OwnedTarget::Uninstaller
        ) {
            return match direction {
                ProductMove::ToPrivate => super::rename_path(&destination, private),
                ProductMove::FromPrivate => super::rename_path(private, &destination),
            };
        }
        let product = product.ok_or_else(|| FilesystemError::CreatedDirectoryChanged {
            path: self.product_directory_path(),
        })?;
        let parent = private
            .parent()
            .ok_or_else(|| FilesystemError::UnsafePath {
                path: private.to_path_buf(),
                reason: "private target has no parent",
            })?;
        let parent = open_directory(parent)
            .map_err(|error| io_failure("open private parent", private, error))?;
        let name = destination
            .file_name()
            .ok_or_else(|| FilesystemError::UnsafePath {
                path: destination.clone(),
                reason: "target has no name",
            })?;
        let private_name = private
            .file_name()
            .ok_or_else(|| FilesystemError::UnsafePath {
                path: private.to_path_buf(),
                reason: "private target has no name",
            })?;
        let result = match direction {
            ProductMove::ToPrivate => rename_exclusive(product, name, &parent, private_name),
            ProductMove::FromPrivate => rename_exclusive(&parent, private_name, product, name),
        };
        result.map_err(|error| io_failure("rename product target", &destination, error))
    }
}

fn product_directory_policy(uid: u32, mode: u32, expected_uid: u32) -> bool {
    uid == expected_uid && mode & 0o022 == 0
}

#[cfg(test)]
mod tests;
