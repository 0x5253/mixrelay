//! macOS distribution transaction planning and unprivileged filesystem staging.
//!
//! `FilesystemAdapter` stages and restores the owned payload paths under an
//! explicit filesystem root. It does not implement launchd, `CoreAudio`, signing, or
//! privilege acquisition. A platform adapter supplies those operations via
//! [`PlatformBackend`], which enforces one ordered transaction,
//! retained shared lease guards, and explicit recovery reporting.
//! Native adapter integration and clean-machine lifecycle verification remain
//! separate release gates.

#[cfg(any(target_os = "macos", test))]
mod activation;
mod backend;
mod contract;
#[cfg(unix)]
mod coordinated;
#[cfg(unix)]
mod directory;
#[cfg(unix)]
mod filesystem;
mod launchd;
#[cfg(unix)]
mod leases;
mod mutation_journal;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
pub mod process_exit;
#[cfg(unix)]
mod release;
#[cfg(target_os = "macos")]
pub mod removal;
#[cfg(any(target_os = "macos", test))]
mod removal_session;
mod request;
mod runtime_definition;
pub mod session;
mod transaction;
mod universal;

pub use backend::{
    LeaseKind, PlatformBackend, Preflight, PreflightError, PreflightResult, RollbackFailure,
};
pub use contract::{
    APP_EXECUTABLE_PATH, APP_INFO_PATH, APP_LEASE_PATH, APP_PATH, DRIVER_PATH, ENGINE_LEASE_PATH,
    HAL_PATH, INSTALLER_LEASE_PATH, LAUNCHD_PATH, LIFECYCLE_DIRECTORY, OwnedTarget,
    PRODUCT_DIRECTORY, REMOVAL_HELPER_PATH, REMOVAL_LAUNCHD_PATH, RUNTIME_ENGINE_PATH,
    RUNTIME_LAUNCHD_PATH, UNINSTALLER_PATH,
};
#[cfg(unix)]
pub use coordinated::{
    CoordinatedBackend, CoordinatedError, CoordinatedRecoveryDiagnostics, ServiceLifecycle,
};
#[cfg(unix)]
pub use filesystem::{
    FilesystemAdapter, FilesystemCommitted, FilesystemError, FilesystemLayout,
    FilesystemRecoveryDiagnostics, FilesystemSnapshot, FilesystemStaged, TargetPresence,
};
pub use launchd::{LaunchdConfiguration, LaunchdError, ServiceAccount};
#[cfg(unix)]
pub use leases::{LeaseError, LifecycleDirectory, LifecycleLeaseGuard};
pub use mutation_journal::FilesystemStageState;
#[cfg(target_os = "macos")]
pub use native::{
    NativeError, NativeServiceLifecycle, NativeServiceRegistration, NativeServiceSnapshot,
    ResolvedServiceAccount, check_coreaudio_response, cleanup_user_data, hal_is_loaded,
    initialize_lifecycle_storage, prepare_install_directories, require_lifecycle_storage,
    require_root, resolve_console_account, resolve_install_account, resolve_removal_account,
};
#[cfg(unix)]
pub use release::{OwnedInstallationSnapshot, ReleaseError, ReleasePayloadVerifier, SigningTeam};
pub use request::{InstallSources, OperationKind, TransactionRequest};
pub use transaction::{
    CleanupStatus, ExecutionResult, RecoveryPhase, RecoveryStatus, TransactionError,
    TransactionOutcome, TransactionState, execute,
};
pub use universal::{UniversalError, validate_universal_binary};
