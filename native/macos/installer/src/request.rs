//! Explicit requests accepted by the lifecycle transaction core.

use std::path::{Path, PathBuf};

/// Prebuilt files supplied to an install transaction.
///
/// These are source inputs only. A backend must validate identifiers, slices,
/// signatures, ownership, and exact destination policy during preflight; this
/// type does not interpret an unsigned manifest or provide destination paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallSources {
    app: PathBuf,
    hal: PathBuf,
    driver: PathBuf,
    launchd: PathBuf,
    remover: PathBuf,
    removal_launchd: PathBuf,
    engine: PathBuf,
    engine_launchd: PathBuf,
}

impl InstallSources {
    /// Creates the complete install source set.
    ///
    /// `driver`, `remover` and `runtime` each pair a prebuilt executable or app
    /// bundle with the launchd definition source that starts it.
    #[must_use]
    pub fn new(
        app: impl Into<PathBuf>,
        hal: impl Into<PathBuf>,
        driver: (impl Into<PathBuf>, impl Into<PathBuf>),
        remover: (impl Into<PathBuf>, impl Into<PathBuf>),
        runtime: (impl Into<PathBuf>, impl Into<PathBuf>),
    ) -> Self {
        Self {
            app: app.into(),
            hal: hal.into(),
            driver: driver.0.into(),
            launchd: driver.1.into(),
            remover: remover.0.into(),
            removal_launchd: remover.1.into(),
            engine: runtime.0.into(),
            engine_launchd: runtime.1.into(),
        }
    }

    /// Returns the audio engine source path.
    #[must_use]
    pub fn runtime_engine(&self) -> &Path {
        &self.engine
    }

    /// Returns the fixed runtime launch template.
    #[must_use]
    pub fn runtime_launchd(&self) -> &Path {
        &self.engine_launchd
    }

    /// Returns the remover app source path.
    #[must_use]
    pub fn remover(&self) -> &Path {
        &self.remover
    }

    /// Returns the fixed root removal helper definition source.
    #[must_use]
    pub fn removal_launchd(&self) -> &Path {
        &self.removal_launchd
    }

    /// Returns the application source path.
    #[must_use]
    pub fn app(&self) -> &Path {
        &self.app
    }

    /// Returns the HAL source path.
    #[must_use]
    pub fn hal(&self) -> &Path {
        &self.hal
    }

    /// Returns the driver source path.
    #[must_use]
    pub fn driver(&self) -> &Path {
        &self.driver
    }

    /// Returns the launchd source path.
    #[must_use]
    pub fn launchd(&self) -> &Path {
        &self.launchd
    }
}

/// The single coordinated operation supported by the installer core.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionRequest {
    /// Replace every exact owned target using the validated sources.
    Install(InstallSources),
    /// Remove only the exact owned targets and their launchd registrations.
    Remove,
}

impl TransactionRequest {
    /// Creates an installation request.
    #[must_use]
    pub fn install(sources: InstallSources) -> Self {
        Self::Install(sources)
    }

    /// Creates a removal request.
    #[must_use]
    pub const fn remove() -> Self {
        Self::Remove
    }

    /// Returns the operation kind without exposing source paths to removal code.
    #[must_use]
    pub const fn kind(&self) -> OperationKind {
        match self {
            Self::Install(_) => OperationKind::Install,
            Self::Remove => OperationKind::Remove,
        }
    }
}

/// The transaction direction selected before any platform operation begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    /// Install or replace the exact owned distribution.
    Install,
    /// Remove only the exact owned distribution.
    Remove,
}
