//! Locates the persisted Driver settings from the service arguments and user environment.

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Context, Result, bail};

pub(crate) fn settings_path(mut arguments: impl Iterator<Item = OsString>) -> Result<PathBuf> {
    if let Some(argument) = arguments.next() {
        if argument != "--settings" {
            bail!(
                "unknown Driver service argument: {}",
                argument.to_string_lossy()
            );
        }
        let path = arguments.next().context("--settings requires a path")?;
        if arguments.next().is_some() {
            bail!("unexpected Driver service arguments");
        }
        return Ok(path.into());
    }
    default_settings_path(
        std::env::var_os("HOME"),
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
    )
}

fn default_settings_path(
    home: Option<OsString>,
    xdg_config: Option<&std::ffi::OsStr>,
) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = xdg_config;
        let home = home.context("Driver service requires the installed user's HOME")?;
        Ok(PathBuf::from(home).join("Library/Application Support/io.mixrelay.driver/drivers.json"))
    }
    #[cfg(target_os = "linux")]
    {
        let directory = if let Some(directory) = xdg_config.filter(|path| !path.is_empty()) {
            PathBuf::from(directory)
        } else {
            PathBuf::from(home.context("Driver service requires HOME or XDG_CONFIG_HOME")?)
                .join(".config")
        };
        if !directory.is_absolute() {
            bail!("Driver configuration directory must be absolute");
        }
        Ok(directory.join("mixrelay-driver/drivers.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_settings_path_does_not_acquire_a_desktop_lease() {
        let path = settings_path(
            [
                OsString::from("--settings"),
                OsString::from("/owned/drivers.json"),
            ]
            .into_iter(),
        )
        .expect("valid settings path");
        assert_eq!(path, PathBuf::from("/owned/drivers.json"));
    }

    #[test]
    fn service_rejects_graph_desktop_ownership_arguments() {
        assert!(settings_path([OsString::from("--desktop-owned")].into_iter()).is_err());
        assert!(settings_path([OsString::from("--settings")].into_iter()).is_err());
        assert!(
            settings_path(
                [
                    OsString::from("--settings"),
                    OsString::from("/owned/drivers.json"),
                    OsString::from("extra")
                ]
                .into_iter()
            )
            .is_err()
        );
    }

    #[test]
    fn default_store_belongs_to_the_service_user() {
        let path = default_settings_path(Some(OsString::from("/Users/driver-user")), None)
            .expect("user directory");
        #[cfg(target_os = "macos")]
        assert_eq!(
            path,
            PathBuf::from(
                "/Users/driver-user/Library/Application Support/io.mixrelay.driver/drivers.json"
            )
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            path,
            PathBuf::from("/Users/driver-user/.config/mixrelay-driver/drivers.json")
        );
        assert!(default_settings_path(None, None).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xdg_store_uses_only_an_absolute_user_configuration_directory() {
        assert_eq!(
            default_settings_path(None, Some(std::ffi::OsStr::new("/user-config")))
                .expect("XDG directory"),
            PathBuf::from("/user-config/mixrelay-driver/drivers.json")
        );
        assert!(default_settings_path(None, Some(std::ffi::OsStr::new("relative"))).is_err());
    }
}
