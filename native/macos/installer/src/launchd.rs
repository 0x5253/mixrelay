//! Closed launchd policy: supplied plists cannot add commands or launch conditions.

use std::{
    io::Cursor,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::DRIVER_PATH;

const SERVICE_LABEL: &str = "io.mixrelay.audio.driver";
const DESKTOP_BUNDLE_IDENTIFIER: &str = "io.mixrelay.desktop";
const MAX_PLIST_BYTES: usize = 16 * 1024;
const TEMPLATE_USER: &str = "__MIXRELAY_SERVICE_USER__";
const TEMPLATE_GROUP: &str = "__MIXRELAY_SERVICE_GROUP__";
const TEMPLATE_HOME: &str = "__MIXRELAY_SERVICE_HOME__";

/// Validated names for one OS-resolved, non-root console account.
///
/// This validates configuration syntax, not account existence or authorization.
/// The privileged caller must match these values to the current OS account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceAccount {
    user: String,
    group: String,
    home: PathBuf,
}

impl ServiceAccount {
    /// Checks account-name and absolute home-path syntax without querying the OS.
    /// # Errors
    /// Returns invalid names, root identity, or a non-normalized/invalid home path.
    pub fn new(user: String, group: String, home: PathBuf) -> Result<Self, LaunchdError> {
        if !valid_account_name(&user) || user == "root" || !valid_account_name(&group) {
            return Err(LaunchdError::Account);
        }
        let Some(home_text) = home.to_str() else {
            return Err(LaunchdError::Account);
        };
        if !home.is_absolute()
            || home == Path::new("/")
            || home_text.len() > 4096
            || home_text.chars().any(char::is_control)
            || home
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(LaunchdError::Account);
        }
        Ok(Self { user, group, home })
    }

    /// Returns the OS-resolved short account name bound into the service plist.
    #[must_use]
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Returns the account's resolved primary group name.
    #[must_use]
    pub fn group(&self) -> &str {
        &self.group
    }

    /// Returns the absolute home path reported by the OS account database.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }
}

fn valid_account_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

/// An exact app-owned launch definition, without arbitrary user-supplied keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchdConfiguration(Definition);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Definition {
    label: String,
    associated_bundle_identifiers: String,
    program_arguments: Vec<String>,
    mach_services: MachServices,
    process_type: String,
    user_name: String,
    group_name: String,
    environment_variables: Environment,
    run_at_load: bool,
    keep_alive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MachServices {
    #[serde(rename = "io.mixrelay.audio.driver")]
    driver: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE", deny_unknown_fields)]
struct Environment {
    home: String,
    user: String,
    logname: String,
}

/// Rejected account syntax, payload policy or property-list encoding.
#[derive(Debug, Error)]
pub enum LaunchdError {
    /// The selected identity cannot form a safe service configuration.
    #[error("invalid non-root service account or home path")]
    Account,
    /// Input size exceeds the fixed configuration budget.
    #[error("launchd configuration exceeds {MAX_PLIST_BYTES} bytes")]
    TooLarge,
    /// The file is not the exact owned launch definition.
    #[error("launchd configuration does not match the fixed MixRelay service policy")]
    Policy,
    /// The closed schema could not be decoded or serialized.
    #[error("invalid launchd property list: {0}")]
    Plist(#[from] plist::Error),
}

impl LaunchdConfiguration {
    /// Extracts the account identity from an already installed fixed-schema plist.
    ///
    /// The returned names and home are only untrusted lookup keys. Native code must
    /// resolve them again through the OS account database and verify numeric UID,
    /// primary GID, home ownership and the rest of this configuration before use.
    /// # Errors
    /// Returns malformed or any non-MixRelay launch definition.
    #[cfg(any(test, target_os = "macos"))]
    pub(crate) fn installed_account(bytes: &[u8]) -> Result<ServiceAccount, LaunchdError> {
        let definition = Self::decode(bytes)?;
        if definition.label != SERVICE_LABEL
            || definition.associated_bundle_identifiers != DESKTOP_BUNDLE_IDENTIFIER
            || definition.program_arguments != [DRIVER_PATH]
            || !definition.mach_services.driver
            || definition.process_type != "Interactive"
            || !definition.run_at_load
            || !definition.keep_alive
        {
            return Err(LaunchdError::Policy);
        }
        let account = ServiceAccount::new(
            definition.user_name.clone(),
            definition.group_name.clone(),
            PathBuf::from(definition.environment_variables.home.clone()),
        )?;
        if definition.environment_variables.user != account.user
            || definition.environment_variables.logname != account.user
        {
            return Err(LaunchdError::Policy);
        }
        Ok(account)
    }

    /// Validates the supplied placeholder template and binds the selected account.
    ///
    /// No supplied command, environment variable or extra launch condition survives
    /// this boundary. The resulting definition is rebuilt from fixed policy.
    /// # Errors
    /// Returns oversized, malformed, additional or modified template fields.
    pub fn from_template(bytes: &[u8], account: &ServiceAccount) -> Result<Self, LaunchdError> {
        let template = Self::decode(bytes)?;
        if template != Definition::new(TEMPLATE_USER, TEMPLATE_GROUP, TEMPLATE_HOME) {
            return Err(LaunchdError::Policy);
        }
        Ok(Self(Definition::for_account(account)))
    }

    /// Validates an installed definition against the already OS-resolved account.
    /// # Errors
    /// Returns any modified, missing, extra, malformed or oversized field.
    pub fn validate_installed(
        bytes: &[u8],
        account: &ServiceAccount,
    ) -> Result<Self, LaunchdError> {
        let definition = Self::decode(bytes)?;
        if definition != Definition::for_account(account) {
            return Err(LaunchdError::Policy);
        }
        Ok(Self(definition))
    }

    /// Serializes this validated definition with XML escaping handled by `plist`.
    /// # Errors
    /// Returns serialization failure or escaped output above the configuration budget.
    pub fn to_xml(&self) -> Result<Vec<u8>, LaunchdError> {
        let mut bytes = Vec::new();
        plist::to_writer_xml(&mut bytes, &self.0)?;
        if bytes.len() > MAX_PLIST_BYTES {
            return Err(LaunchdError::TooLarge);
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Definition, LaunchdError> {
        if bytes.len() > MAX_PLIST_BYTES {
            return Err(LaunchdError::TooLarge);
        }
        Ok(plist::from_reader(Cursor::new(bytes))?)
    }
}

impl Definition {
    fn for_account(account: &ServiceAccount) -> Self {
        // Construction accepts only UTF-8 homes; no path interpolation enters a shell.
        Self::new(
            &account.user,
            &account.group,
            &account.home.to_string_lossy(),
        )
    }

    fn new(user: &str, group: &str, home: &str) -> Self {
        Self {
            label: SERVICE_LABEL.to_owned(),
            associated_bundle_identifiers: DESKTOP_BUNDLE_IDENTIFIER.to_owned(),
            program_arguments: vec![DRIVER_PATH.to_owned()],
            mach_services: MachServices { driver: true },
            process_type: "Interactive".to_owned(),
            user_name: user.to_owned(),
            group_name: group.to_owned(),
            environment_variables: Environment {
                home: home.to_owned(),
                user: user.to_owned(),
                logname: user.to_owned(),
            },
            run_at_load: true,
            keep_alive: true,
        }
    }
}

#[cfg(test)]
mod tests;
