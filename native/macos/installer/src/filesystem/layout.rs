//! Where each owned target lives, and the private staging tree that holds its copies.

use std::path::{Path, PathBuf};

use super::{
    error::FilesystemError,
    paths::{ensure_no_symlink_components, require_directory, validate_absolute_path},
};
use crate::contract::{LIFECYCLE_DIRECTORY, OwnedTarget};

const STAGING_DIRECTORY_NAME: &str = ".installer-staging";
const PAYLOAD_DIRECTORY_NAME: &str = "payload";
const BACKUP_DIRECTORY_NAME: &str = "backups";

/// The expected filesystem kind of an owned destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TargetKind {
    Directory,
    File,
}

impl TargetKind {
    pub(super) fn of(target: OwnedTarget) -> Self {
        match target {
            OwnedTarget::App | OwnedTarget::Hal | OwnedTarget::Uninstaller => Self::Directory,
            OwnedTarget::Driver
            | OwnedTarget::Launchd
            | OwnedTarget::RemovalLaunchd
            | OwnedTarget::RuntimeEngine
            | OwnedTarget::RuntimeLaunchd => Self::File,
        }
    }

    pub(super) fn matches(self, metadata: &std::fs::Metadata) -> bool {
        match self {
            Self::Directory => metadata.is_dir(),
            Self::File => metadata.is_file(),
        }
    }
}

/// The name of a target's copy under `payload/` and `backups/` in private staging.
pub(super) fn staging_name(target: OwnedTarget) -> &'static str {
    match target {
        OwnedTarget::App => "app",
        OwnedTarget::Driver => "driver",
        OwnedTarget::Hal => "hal",
        OwnedTarget::Launchd => "launchd",
        OwnedTarget::Uninstaller => "uninstaller",
        OwnedTarget::RemovalLaunchd => "removal-launchd",
        OwnedTarget::RuntimeEngine => "runtime-engine",
        OwnedTarget::RuntimeLaunchd => "runtime-launchd",
    }
}

/// App bundles inside the product folder, which move through its retained descriptor.
pub(super) fn in_product_directory(target: OwnedTarget) -> bool {
    matches!(target, OwnedTarget::App | OwnedTarget::Uninstaller)
}

pub(super) fn payload_root(staging_root: &Path) -> PathBuf {
    staging_root.join(PAYLOAD_DIRECTORY_NAME)
}

pub(super) fn backup_root(staging_root: &Path) -> PathBuf {
    staging_root.join(BACKUP_DIRECTORY_NAME)
}

/// An absolute root used to map the fixed production destinations into a fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemLayout {
    root: PathBuf,
    staging_root: PathBuf,
}

impl FilesystemLayout {
    /// Creates a layout rooted at `root`; `/` is the production root.
    ///
    /// The root must already exist and contain no symlink component. The
    /// lifecycle parent may be created by a privileged preflight, but this
    /// adapter will not create it on its own.
    ///
    /// # Errors
    /// Returns an unsafe-path or filesystem error when `root` is not a safe directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, FilesystemError> {
        let root = root.into();
        validate_absolute_path(&root)?;
        ensure_no_symlink_components(&root, false)?;
        require_directory(&root, "filesystem root")?;
        let lifecycle = root.join(LIFECYCLE_DIRECTORY.trim_start_matches('/'));
        let staging_root = lifecycle.join(STAGING_DIRECTORY_NAME);
        Ok(Self { root, staging_root })
    }

    /// Returns the absolute fixture or production root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the private transaction staging path.
    #[must_use]
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    pub(crate) fn target_path(&self, target: OwnedTarget) -> PathBuf {
        self.root.join(target.path().trim_start_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_has_a_distinct_staging_name_and_a_kind_matching_its_path() {
        let names = OwnedTarget::all().map(staging_name);
        for (index, name) in names.iter().enumerate() {
            assert!(!names[index + 1..].contains(name), "duplicate {name}");
        }
        for target in OwnedTarget::all() {
            let bundle = Path::new(target.path())
                .extension()
                .is_some_and(|extension| extension == "app" || extension == "driver");
            assert_eq!(TargetKind::of(target) == TargetKind::Directory, bundle);
        }
    }

    #[test]
    fn only_the_two_app_bundles_live_in_the_product_folder() {
        for target in OwnedTarget::all() {
            let parent = Path::new(target.path()).parent();
            assert_eq!(
                in_product_directory(target),
                parent == Some(Path::new(crate::contract::PRODUCT_DIRECTORY)),
                "{target:?}"
            );
        }
    }
}
