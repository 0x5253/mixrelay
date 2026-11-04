//! Validates and prepares the fixed directories that receive installed targets.

use std::{fs::File, path::Path};

use rustix::fs::Uid;

use super::{
    APP_DIRECTORY, admin_gid,
    anchored::{
        inspect, inspect_path, inspect_path_if_present, io_error, open_child_directory,
        open_or_create_root_directory, open_protected_directory, open_root, require_root_directory,
    },
    policy::{self, ParentRule},
    product, require, wheel_group,
};
use crate::{
    LIFECYCLE_DIRECTORY, PRODUCT_DIRECTORY,
    native::{NativeError, privilege::require_root},
};

const HAL_DIRECTORY: &str = "/Library/Audio/Plug-Ins/HAL";
const LAUNCH_DAEMONS_DIRECTORY: &str = "/Library/LaunchDaemons";

/// Creates the system HAL plug-in directory when it is missing. A fresh macOS has no
/// `/Library/Audio/Plug-Ins/HAL` until some audio driver installs one. Missing levels are
/// created as root:wheel 0755, as macOS does; existing ones are left unchanged and
/// `validate_destination_ancestors` then rejects unsafe ownership or modes.
/// # Errors
/// Returns when a level cannot be opened or created without following links.
pub fn prepare_install_directories() -> Result<(), NativeError> {
    require_root()?;
    let wheel = wheel_group()?;
    let library = open_child_directory(&open_root()?, "Library", Path::new("/Library"))?;
    let audio =
        open_or_create_root_directory(&library, "Audio", Path::new("/Library/Audio"), wheel)?;
    let plugins = open_or_create_root_directory(
        &audio,
        "Plug-Ins",
        Path::new("/Library/Audio/Plug-Ins"),
        wheel,
    )?;
    open_or_create_root_directory(&plugins, "HAL", Path::new(HAL_DIRECTORY), wheel)?;
    Ok(())
}

/// Publishes a privately configured product folder into Applications exclusively.
pub(crate) fn create_product_directory() -> Result<File, NativeError> {
    require_root()?;
    let path = Path::new(PRODUCT_DIRECTORY);
    let applications_path = path.parent().ok_or_else(|| NativeError::Trust {
        path: path.to_path_buf(),
        reason: "product folder has no Applications parent",
    })?;
    let applications = open_protected_directory(applications_path)?;
    let inode = inspect(
        &applications,
        "inspect Applications directory",
        applications_path,
    )?;
    require(
        applications_path,
        policy::applications_directory(inode, admin_gid()?),
    )?;
    let wheel = wheel_group()?;
    let lifecycle = open_protected_directory(Path::new(LIFECYCLE_DIRECTORY))?;
    require_root_directory(Path::new(LIFECYCLE_DIRECTORY), &lifecycle, Some(wheel))?;
    let directory =
        product::create_product_directory_at(&applications, &lifecycle, (Uid::ROOT, wheel), || {})
            .map_err(|source| io_error("publish protected product folder", path, source))?;
    require_root_directory(path, &directory, Some(wheel))?;
    Ok(directory)
}

/// Revalidates every existing directory leading to an install target without changing it.
pub(crate) fn validate_destination_ancestors(root: &Path) -> Result<(), NativeError> {
    if root != Path::new("/") {
        return Err(NativeError::Trust {
            path: root.to_path_buf(),
            reason: "native service operations require the production filesystem root",
        });
    }
    let applications = ParentRule::Applications {
        admin_gid: admin_gid()?,
    };
    let parents = [
        ("/Applications", applications),
        ("/Library/Application Support", ParentRule::Protected),
        (APP_DIRECTORY, ParentRule::Protected),
        (HAL_DIRECTORY, ParentRule::Protected),
        (LAUNCH_DAEMONS_DIRECTORY, ParentRule::Protected),
    ];
    for (directory, rule) in parents {
        let directory = Path::new(directory);
        let opened = open_protected_directory(directory)?;
        let inode = inspect(&opened, "inspect destination parent", directory)?;
        require(directory, policy::destination_parent(inode, rule))?;
    }
    let product = Path::new(PRODUCT_DIRECTORY);
    if inspect_path_if_present("inspect product folder", product)?.is_some() {
        open_protected_directory(product)?;
    }
    validate_engine_directory(root, false)
}

/// Checks the one engine parent, allowing it to be absent only during preflight.
pub(in crate::native) fn validate_engine_directory(
    root: &Path,
    required: bool,
) -> Result<(), NativeError> {
    let path = root.join("Library/Application Support/MixRelay/bin");
    let inode = if required {
        Some(inspect_path("inspect engine parent", &path)?)
    } else {
        inspect_path_if_present("inspect engine parent", &path)?
    };
    let Some(inode) = inode else {
        return Ok(());
    };
    require(
        &path,
        policy::engine_directory(inode, wheel_group()?.as_raw()),
    )
}

/// Requires exact root ownership and closed permissions on one existing target.
pub(in crate::native) fn validate_owned_target(path: &Path, mode: u32) -> Result<(), NativeError> {
    let inode = inspect_path("inspect owned target", path)?;
    require(path, policy::owned_target(inode, mode))
}
