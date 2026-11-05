//! macOS-only privileged adapter for account, trust and launchd boundaries.

mod account;
mod audio_probe;
mod command;
mod hal_reload;
mod launchctl;
mod privilege;
mod service;
mod trust;
mod user_data;

pub use account::{ResolvedServiceAccount, resolve_console_account};
pub use audio_probe::{check_coreaudio_response, hal_is_loaded};
pub(crate) use launchctl::{REMOVAL_HELPER, RegistrationState, bootout, bootstrap, inspect};
pub use privilege::require_root;
pub use service::{NativeServiceLifecycle, NativeServiceRegistration, NativeServiceSnapshot};
pub(crate) use trust::destination::{create_product_directory, validate_destination_ancestors};
pub use trust::{
    destination::prepare_install_directories,
    lifecycle::{initialize_lifecycle_storage, require_lifecycle_storage},
};
pub use user_data::cleanup_user_data;

use crate::LaunchdConfiguration;

/// Selects the already-installed service account for upgrade, or the actual
/// console account for a fresh install. Existing malformed state fails closed.
/// # Errors
/// Returns when the installed plist is untrusted, malformed or not backed by a
/// matching passwd/group/home record.
pub fn resolve_install_account() -> Result<ResolvedServiceAccount, NativeError> {
    require_root()?;
    match trust::installed_plist::read_installed_launchd_plist()? {
        Some(bytes) => {
            let account = LaunchdConfiguration::installed_account(&bytes)?;
            account::resolve_service_account(&account)
        }
        None => resolve_console_account(),
    }
}

/// Resolves the installed account, retaining its lease owner when a user has
/// already deleted the launchd plist. With no installation identity, uses the console.
/// # Errors
/// Returns when a present plist or lease is untrusted, or the account cannot be
/// revalidated against the operating system.
pub fn resolve_removal_account() -> Result<ResolvedServiceAccount, NativeError> {
    require_root()?;
    if let Some(bytes) = trust::installed_plist::read_installed_launchd_plist()? {
        let account = LaunchdConfiguration::installed_account(&bytes)?;
        return account::resolve_service_account(&account);
    }
    match trust::lifecycle::read_lifecycle_account()? {
        Some(account) => Ok(account),
        None => resolve_console_account(),
    }
}

use std::{io, path::PathBuf};

use thiserror::Error;

use crate::{LaunchdError, LeaseError, ReleaseError};

/// Fail-closed native installation error with bounded, escaped child diagnostics.
#[derive(Debug, Error)]
pub enum NativeError {
    /// Installing or removing the HAL requires explicit audio-service interruption consent.
    #[error("install/remove require --reload-coreaudio approval; active audio will be interrupted")]
    CoreAudioReloadApprovalRequired,
    /// Native HAL activation or Driver readiness could not be established.
    #[error("native audio activation failed: {0}")]
    Activation(String),
    /// Unprivileged Driver settings cleanup failed; the installation is rolled back.
    #[error("Driver settings cleanup failed: {0}")]
    UserData(#[source] anyhow::Error),
    /// A mutating installer operation was invoked without effective root:wheel.
    #[error(
        "administrator approval must run as root with the wheel group; this command never escalates privileges"
    )]
    RootRequired,
    /// A selected console/service identity could not be fully resolved or revalidated.
    #[error("cannot resolve and validate the active console account: {0}")]
    Account(String),
    /// A fixed parent or target violated the trusted ownership/mode policy.
    #[error("untrusted native path {path}: {reason}")]
    Trust {
        /// Path whose ownership, mode, type or identity was rejected.
        path: PathBuf,
        /// Compact invariant description.
        reason: &'static str,
    },
    /// A fixed native process could not be launched or reaped safely.
    #[error("native command {program} failed: {source}")]
    CommandIo {
        /// Absolute executable used by the adapter.
        program: &'static str,
        #[source]
        source: io::Error,
    },
    /// A bounded native child returned an unrecognized result.
    #[error("native command {program} returned {status}: {detail}")]
    CommandResult {
        /// Absolute executable used by the adapter.
        program: &'static str,
        /// Exit code or terminating signal.
        status: String,
        /// Escaped, bounded stdout/stderr context.
        detail: String,
    },
    /// launchctl output did not match the closed known schema.
    #[error("launchd registration state is unknown: {0}")]
    LaunchdState(&'static str),
    /// An operation failed while accessing a fixed native path.
    #[error("native filesystem {operation} failed for {path}: {source}")]
    Io {
        /// Operation attempted.
        operation: &'static str,
        /// Fixed path involved.
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The account-bound plist did not satisfy the closed service contract.
    #[error(transparent)]
    LaunchdPolicy(#[from] LaunchdError),
    /// Signature, bundle identity, architecture or release payload verification failed.
    #[error(transparent)]
    Payload(#[from] ReleaseError),
    /// Lifecycle lease validation or acquisition failed.
    #[error(transparent)]
    Lease(#[from] LeaseError),
    /// Exact-target staging or rollback failed.
    #[error(transparent)]
    Filesystem(#[from] crate::FilesystemError),
}
