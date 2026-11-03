//! Fixed paths owned by the `MixRelay` administrator-installed distribution.

include!(concat!(env!("OUT_DIR"), "/product_paths.rs"));
/// The persistent Driver service binary owned by the installer.
pub const DRIVER_PATH: &str = "/Library/Application Support/MixRelay/bin/mixrelay-driver-service";
/// The HAL bundle owned by the installer.
pub const HAL_PATH: &str = "/Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver";
/// The launchd configuration owned by the installer.
pub const LAUNCHD_PATH: &str = "/Library/LaunchDaemons/io.mixrelay.audio.driver.plist";
/// The on-demand, fixed-operation removal helper's launchd definition.
pub const REMOVAL_LAUNCHD_PATH: &str = "/Library/LaunchDaemons/io.mixrelay.driver.removal.plist";
/// The root-owned directory containing the three lifecycle lease inodes.
pub const LIFECYCLE_DIRECTORY: &str = "/Library/Application Support/MixRelay/lifecycle";
/// The fixed desktop ownership lease inode.
pub const APP_LEASE_PATH: &str = "/Library/Application Support/MixRelay/lifecycle/app.owner";
/// The fixed engine ownership lease inode.
pub const ENGINE_LEASE_PATH: &str = "/Library/Application Support/MixRelay/lifecycle/engine.owner";
/// The fixed exclusive installer-transaction lease inode.
pub const INSTALLER_LEASE_PATH: &str =
    "/Library/Application Support/MixRelay/lifecycle/installer.owner";

/// The Driver-owned audio engine executable.
pub const RUNTIME_ENGINE_PATH: &str = "/Library/Application Support/MixRelay/bin/mixrelay-service";
/// The engine's code-signing identifier. It equals `CFBundleIdentifier` in
/// `native/macos/service/Info.plist`; `scripts/macos-identity.test.mjs` checks every copy.
pub(crate) const RUNTIME_ENGINE_IDENTIFIER: &str = "io.mixrelay.service";
/// The Driver-owned on-demand engine definition.
pub const RUNTIME_LAUNCHD_PATH: &str = "/Library/LaunchDaemons/io.mixrelay.audio.engine.plist";

/// One exact path that a coordinated transaction may replace or remove.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedTarget {
    /// The application bundle.
    App,
    /// The persistent Driver service binary.
    Driver,
    /// The HAL bundle.
    Hal,
    /// The launchd property list.
    Launchd,
    /// The separate removal app and its embedded privileged executable.
    Uninstaller,
    /// The on-demand removal helper definition.
    RemovalLaunchd,
    /// The audio engine owned by Driver.
    RuntimeEngine,
    /// The on-demand audio engine definition.
    RuntimeLaunchd,
}

impl OwnedTarget {
    /// Returns the only absolute path associated with this owned target.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::App => APP_PATH,
            Self::Driver => DRIVER_PATH,
            Self::Hal => HAL_PATH,
            Self::Launchd => LAUNCHD_PATH,
            Self::Uninstaller => UNINSTALLER_PATH,
            Self::RemovalLaunchd => REMOVAL_LAUNCHD_PATH,
            Self::RuntimeEngine => RUNTIME_ENGINE_PATH,
            Self::RuntimeLaunchd => RUNTIME_LAUNCHD_PATH,
        }
    }

    /// Returns every replaceable target in deterministic transaction order.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::App,
            Self::Driver,
            Self::Hal,
            Self::Launchd,
            Self::Uninstaller,
            Self::RemovalLaunchd,
            Self::RuntimeEngine,
            Self::RuntimeLaunchd,
        ]
    }
}
