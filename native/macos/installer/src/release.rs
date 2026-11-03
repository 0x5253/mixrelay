//! Ownership and launch verification for installed code and release payloads.
//!
//! Existing targets inspected before install/update or removal use ownership:
//! fixed bundle/executable identity, Developer ID team requirement, valid signature,
//! Universal 2, Hardened Runtime and secure timestamp. Their entitlements are unrestricted.
//! Sources, private staged payloads and new installed code before registration or
//! reopening apps use launch policy with the exact role entitlement allowlist.
//! This includes the engine, HAL, Driver, uninstaller and embedded installer helper.
//! Recovery reactivates previous code with ownership and equality to the captured
//! installation snapshot. Previous versions may have different entitlements.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use rustix::fs::{Mode, OFlags};
use serde::Deserialize;
use thiserror::Error;

use crate::{
    FilesystemLayout, FilesystemStaged, InstallSources, LaunchdConfiguration, LaunchdError,
    OwnedTarget, ServiceAccount, UniversalError, contract::RUNTIME_ENGINE_IDENTIFIER,
    validate_universal_binary,
};

mod policy;
mod recovery;
pub use recovery::OwnedInstallationSnapshot;
#[cfg(target_os = "macos")]
mod security_metadata;
mod signature;
#[cfg(any(target_os = "macos", test))]
use policy::{Architecture, PolicyViolation};
use policy::{ArtifactRole, VerificationPolicy};
use signature::verify_signature;

/// The publisher identity required for every executable in a release payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SigningTeam(String);

impl SigningTeam {
    /// Validates a ten-character Apple Developer Team identifier.
    /// # Errors
    /// Returns invalid syntax. This does not assert membership or certificate availability.
    pub fn new(value: String) -> Result<Self, ReleaseError> {
        if value.len() != 10
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(ReleaseError::SigningTeam);
        }
        Ok(Self(value))
    }

    /// Returns the validated publisher Team ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn requirement(&self, identifier: &str) -> String {
        // Identifiers come only from the fixed artifact policy below. Team syntax
        // excludes requirement-language metacharacters; no shell is invoked.
        format!(
            "anchor apple generic and identifier \"{identifier}\" and certificate leaf[subject.OU] = \"{}\" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
            self.0
        )
    }
}

/// Native release payload failure; no installed artifact is changed by verification.
#[derive(Debug, Error)]
pub enum ReleaseError {
    /// Recovery targets differ from the captured filesystem state.
    #[error(transparent)]
    Filesystem(#[from] crate::FilesystemError),
    /// A recovery snapshot was captured for a different publisher or account.
    #[error("installation recovery snapshot belongs to another publisher or account")]
    SnapshotIdentity,
    /// A file inside an installed bundle changed since the snapshot.
    #[error("restored installation entries differ from their preflight snapshot")]
    SnapshotContents,
    /// Invalid publisher identifier.
    #[error("expected a ten-character uppercase Apple Developer Team identifier")]
    SigningTeam,
    /// A source argument could be interpreted as an option or ambiguous path.
    #[error("release source must be an absolute path: {0}")]
    SourcePath(PathBuf),
    /// File access, launch or child reaping failed.
    #[error("release verification I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Input is not a bounded regular configuration file.
    #[error("invalid or oversized release configuration: {0}")]
    ConfigurationFile(PathBuf),
    /// The application/HAL identity or executable differs from fixed policy.
    #[error("unexpected bundle identifier or executable: {0}")]
    BundleIdentity(PathBuf),
    /// Bundle metadata could not be decoded.
    #[error("invalid bundle metadata: {0}")]
    BundlePlist(#[from] plist::Error),
    /// A binary did not contain the required architecture slices.
    #[error("invalid Universal 2 binary {path}: {source}")]
    Universal {
        path: PathBuf,
        #[source]
        source: UniversalError,
    },
    /// The launchd payload does not match fixed policy.
    #[error(transparent)]
    Launchd(#[from] LaunchdError),
    /// Apple rejected integrity, architecture or publisher identity.
    #[error("Developer ID signature verification failed for {path}: {status}")]
    Signature { path: PathBuf, status: String },
    /// Structured signing metadata did not satisfy a release artifact policy.
    #[error("invalid {architecture} signing metadata for {path}: {reason}")]
    SignaturePolicy {
        path: PathBuf,
        architecture: &'static str,
        reason: &'static str,
    },
    /// Apple Security.framework could not read a specific architecture's signing metadata.
    #[error("could not inspect {architecture} signing metadata for {path}: {status}")]
    SignatureMetadata {
        path: PathBuf,
        architecture: &'static str,
        status: String,
    },
    /// A verification subprocess exceeded its deadline and was reaped.
    #[error("signature verification timed out for {0}")]
    Timeout(PathBuf),
    /// Native Apple signature checking is unavailable on this OS.
    #[error("Developer ID verification requires macOS")]
    UnsupportedPlatform,
}

/// Verifies actual copies using Apple signature checking and fixed product policy.
///
/// This never signs code, registers services, modifies installed files, or acquires
/// privileges. Native install callers still establish root ownership and validate
/// the selected account against the OS before using the returned launch definition.
#[derive(Debug)]
pub struct ReleasePayloadVerifier {
    team: SigningTeam,
    account: ServiceAccount,
}

impl ReleasePayloadVerifier {
    /// Binds one trusted publisher and OS-resolved service identity.
    #[must_use]
    pub const fn new(team: SigningTeam, account: ServiceAccount) -> Self {
        Self { team, account }
    }

    /// Verifies explicitly supplied artifacts, returning a bound launch definition.
    ///
    /// This preflight is not proof that later copies are identical. Always verify
    /// the private copies again through [`Self::verify_staged`] before committing.
    /// # Errors
    /// Returns invalid signatures, architectures, bundle identities or launch policy.
    pub fn verify_sources(
        &self,
        sources: &InstallSources,
    ) -> Result<LaunchdConfiguration, ReleaseError> {
        self.verify_with(sources, true, verify_signature)
    }

    /// Verifies new installed code before registration or reopening apps.
    ///
    /// Every `required` target must be present, and every present target passes
    /// launch policy, including exact entitlements. A missing target
    /// that is not required, such as the remover of a partial installation, is allowed.
    /// # Errors
    /// Returns a missing required target or an invalid present target.
    pub fn verify_installed(
        &self,
        layout: &FilesystemLayout,
        required: &[OwnedTarget],
    ) -> Result<(), ReleaseError> {
        self.verify_installed_with(layout, required, verify_signature)
    }

    fn verify_installed_with(
        &self,
        layout: &FilesystemLayout,
        required: &[OwnedTarget],
        signature: impl FnMut(&Path, &str, ArtifactRole, VerificationPolicy) -> Result<(), ReleaseError>,
    ) -> Result<(), ReleaseError> {
        Self::require_targets(layout, required)?;
        self.verify_present_with(layout, VerificationPolicy::Launch, signature)
            .map(|_| ())
    }

    fn require_targets(
        layout: &FilesystemLayout,
        required: &[OwnedTarget],
    ) -> Result<(), ReleaseError> {
        for target in required {
            let path = layout.root().join(target.path().trim_start_matches('/'));
            if let Err(source) = std::fs::symlink_metadata(&path) {
                return Err(ReleaseError::Io { path, source });
            }
        }
        Ok(())
    }

    /// Verifies ownership of each present target before replacement or removal.
    ///
    /// Missing targets permit repair or repeated removal. Present code must retain
    /// its fixed identity and publisher signature, architectures, Hardened Runtime
    /// and secure timestamp. Entitlements do not affect ownership.
    /// # Errors
    /// Returns invalid present files, identities, signatures or ownership metadata.
    pub fn verify_ownership(
        &self,
        layout: &FilesystemLayout,
    ) -> Result<Option<LaunchdConfiguration>, ReleaseError> {
        self.verify_ownership_with(layout, verify_signature)
    }

    fn verify_ownership_with(
        &self,
        layout: &FilesystemLayout,
        signature: impl FnMut(&Path, &str, ArtifactRole, VerificationPolicy) -> Result<(), ReleaseError>,
    ) -> Result<Option<LaunchdConfiguration>, ReleaseError> {
        self.verify_present_with(layout, VerificationPolicy::Ownership, signature)
    }

    fn verify_installed_bundle(
        &self,
        target: OwnedTarget,
        path: &Path,
        policy: VerificationPolicy,
        signature: &mut impl FnMut(
            &Path,
            &str,
            ArtifactRole,
            VerificationPolicy,
        ) -> Result<(), ReleaseError>,
    ) -> Result<(), ReleaseError> {
        let (identifier, executable, role) = match target {
            OwnedTarget::App => (
                "io.mixrelay.desktop",
                "mixrelay-desktop",
                ArtifactRole::Application,
            ),
            OwnedTarget::Hal => ("io.mixrelay.audio.driver", "MixRelayHAL", ArtifactRole::Hal),
            _ => (
                "io.mixrelay.driver.uninstaller",
                "mixrelay-driver-uninstaller",
                ArtifactRole::Uninstaller,
            ),
        };
        verify_bundle(path, identifier, executable)?;
        signature(path, &self.team.requirement(identifier), role, policy)?;
        if target == OwnedTarget::Uninstaller {
            self.verify_installer_executable_with(
                &path.join("Contents/Helpers/mixrelay-macos-installer"),
                policy,
                signature,
            )?;
        }
        Ok(())
    }

    fn verify_present_with(
        &self,
        layout: &FilesystemLayout,
        policy: VerificationPolicy,
        mut signature: impl FnMut(
            &Path,
            &str,
            ArtifactRole,
            VerificationPolicy,
        ) -> Result<(), ReleaseError>,
    ) -> Result<Option<LaunchdConfiguration>, ReleaseError> {
        let mut definition = None;
        for target in OwnedTarget::all() {
            let path = layout.root().join(target.path().trim_start_matches('/'));
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(ReleaseError::Io { path, source }),
            }
            match target {
                OwnedTarget::App | OwnedTarget::Hal | OwnedTarget::Uninstaller => {
                    self.verify_installed_bundle(target, &path, policy, &mut signature)?;
                }
                OwnedTarget::RuntimeEngine => {
                    verify_architecture(&path)?;
                    signature(
                        &path,
                        &self.team.requirement(RUNTIME_ENGINE_IDENTIFIER),
                        ArtifactRole::Engine,
                        policy,
                    )?;
                }
                OwnedTarget::RuntimeLaunchd => {
                    crate::runtime_definition::validate_installed(
                        &read_configuration(&path, 16 * 1024)?,
                        &self.account,
                    )
                    .map_err(|source| ReleaseError::Io {
                        path: path.clone(),
                        source: io::Error::other(source.to_string()),
                    })?;
                }
                OwnedTarget::Driver => {
                    verify_architecture(&path)?;
                    signature(
                        &path,
                        &self.team.requirement("mixrelay-driver-service"),
                        ArtifactRole::Driver,
                        policy,
                    )?;
                }
                OwnedTarget::Launchd => {
                    definition = Some(LaunchdConfiguration::validate_installed(
                        &read_configuration(&path, 16 * 1024)?,
                        &self.account,
                    )?);
                }
                OwnedTarget::RemovalLaunchd => {
                    #[cfg(target_os = "macos")]
                    crate::removal::verify_removal_launchd(&path, &self.team).map_err(|error| {
                        ReleaseError::Io {
                            path: path.clone(),
                            source: io::Error::other(error.to_string()),
                        }
                    })?;
                }
            }
        }
        Ok(definition)
    }

    /// Verifies private install copies, not the original source paths.
    /// # Errors
    /// Returns absent/removal payloads or failed signature, format or policy checks.
    pub fn verify_staged(
        &self,
        staged: &FilesystemStaged,
    ) -> Result<LaunchdConfiguration, ReleaseError> {
        self.verify_staged_with(staged, verify_signature)
    }

    fn verify_staged_with(
        &self,
        staged: &FilesystemStaged,
        signature: impl FnMut(&Path, &str, ArtifactRole, VerificationPolicy) -> Result<(), ReleaseError>,
    ) -> Result<LaunchdConfiguration, ReleaseError> {
        let path = |target| staged.payload_path(target);
        let sources = InstallSources::new(
            path(OwnedTarget::App),
            path(OwnedTarget::Hal),
            (path(OwnedTarget::Driver), path(OwnedTarget::Launchd)),
            (
                path(OwnedTarget::Uninstaller),
                path(OwnedTarget::RemovalLaunchd),
            ),
            (
                path(OwnedTarget::RuntimeEngine),
                path(OwnedTarget::RuntimeLaunchd),
            ),
        );
        self.verify_with(&sources, true, signature)
    }

    /// Verifies the separately installed remover and its embedded fixed CLI.
    /// # Errors
    /// Returns an invalid bundle, executable, signature or publisher identity.
    pub fn verify_uninstaller_bundle(&self, bundle: impl AsRef<Path>) -> Result<(), ReleaseError> {
        let bundle = bundle.as_ref();
        verify_bundle(
            bundle,
            "io.mixrelay.driver.uninstaller",
            "mixrelay-driver-uninstaller",
        )?;
        verify_signature(
            bundle,
            &self.team.requirement("io.mixrelay.driver.uninstaller"),
            ArtifactRole::Uninstaller,
            VerificationPolicy::Launch,
        )?;
        self.verify_installer_executable(bundle.join("Contents/Helpers/mixrelay-macos-installer"))
    }

    /// Verifies the remover and embedded helper for deletion without launching them.
    #[cfg(target_os = "macos")]
    pub(crate) fn verify_uninstaller_ownership(&self, bundle: &Path) -> Result<(), ReleaseError> {
        self.verify_installed_bundle(
            OwnedTarget::Uninstaller,
            bundle,
            VerificationPolicy::Ownership,
            &mut verify_signature,
        )
    }

    /// Verifies the signed universal installer helper before it is embedded in a package.
    ///
    /// Requires its fixed Developer ID identity, Universal 2, Hardened Runtime,
    /// secure timestamps and an empty entitlement allowlist on both slices.
    /// # Errors
    /// Returns an invalid path, architecture, signature, signing policy, or Security.framework
    /// metadata error.
    pub fn verify_installer_executable(&self, path: impl AsRef<Path>) -> Result<(), ReleaseError> {
        self.verify_installer_executable_with(
            path.as_ref(),
            VerificationPolicy::Launch,
            verify_signature,
        )
    }

    fn verify_installer_executable_with(
        &self,
        path: &Path,
        policy: VerificationPolicy,
        signature: impl FnOnce(
            &Path,
            &str,
            ArtifactRole,
            VerificationPolicy,
        ) -> Result<(), ReleaseError>,
    ) -> Result<(), ReleaseError> {
        if !path.is_absolute() {
            return Err(ReleaseError::SourcePath(path.to_path_buf()));
        }
        verify_architecture(path)?;
        signature(
            path,
            &self.team.requirement("mixrelay-macos-installer"),
            ArtifactRole::Installer,
            policy,
        )
    }

    fn verify_runtime_with(
        &self,
        sources: &InstallSources,
        is_template: bool,
        mut signature: impl FnMut(
            &Path,
            &str,
            ArtifactRole,
            VerificationPolicy,
        ) -> Result<(), ReleaseError>,
    ) -> Result<(), ReleaseError> {
        let (engine, plist) = (sources.runtime_engine(), sources.runtime_launchd());
        verify_architecture(engine)?;
        signature(
            engine,
            &self.team.requirement(RUNTIME_ENGINE_IDENTIFIER),
            ArtifactRole::Engine,
            VerificationPolicy::Launch,
        )?;
        let bytes = read_configuration(plist, 16 * 1024)?;
        let result = if is_template {
            crate::runtime_definition::from_template(&bytes, &self.account)
        } else {
            crate::runtime_definition::validate_installed(&bytes, &self.account)
        };
        result.map_err(|source| ReleaseError::Io {
            path: plist.to_path_buf(),
            source: io::Error::other(source.to_string()),
        })?;
        Ok(())
    }

    fn verify_with(
        &self,
        sources: &InstallSources,
        is_template: bool,
        mut signature: impl FnMut(
            &Path,
            &str,
            ArtifactRole,
            VerificationPolicy,
        ) -> Result<(), ReleaseError>,
    ) -> Result<LaunchdConfiguration, ReleaseError> {
        for path in [
            sources.app(),
            sources.hal(),
            sources.driver(),
            sources.launchd(),
            sources.remover(),
            sources.removal_launchd(),
            sources.runtime_engine(),
            sources.runtime_launchd(),
        ] {
            if !path.is_absolute() {
                return Err(ReleaseError::SourcePath(path.to_path_buf()));
            }
        }
        let launchd = read_configuration(sources.launchd(), 16 * 1024)?;
        let definition = if is_template {
            LaunchdConfiguration::from_template(&launchd, &self.account)?
        } else {
            LaunchdConfiguration::validate_installed(&launchd, &self.account)?
        };
        self.verify_runtime_with(sources, is_template, &mut signature)?;
        let app = sources.remover();
        signature(
            app,
            &self.team.requirement("io.mixrelay.driver.uninstaller"),
            ArtifactRole::Uninstaller,
            VerificationPolicy::Launch,
        )?;
        verify_bundle(
            app,
            "io.mixrelay.driver.uninstaller",
            "mixrelay-driver-uninstaller",
        )?;
        self.verify_installer_executable_with(
            &app.join("Contents/Helpers/mixrelay-macos-installer"),
            VerificationPolicy::Launch,
            &mut signature,
        )?;
        #[cfg(target_os = "macos")]
        crate::removal::verify_removal_launchd(sources.removal_launchd(), &self.team).map_err(
            |error| ReleaseError::Io {
                path: sources.removal_launchd().to_path_buf(),
                source: std::io::Error::other(error.to_string()),
            },
        )?;
        for (bundle, identifier, executable, role) in [
            (
                sources.app(),
                "io.mixrelay.desktop",
                "mixrelay-desktop",
                ArtifactRole::Application,
            ),
            (
                sources.hal(),
                "io.mixrelay.audio.driver",
                "MixRelayHAL",
                ArtifactRole::Hal,
            ),
        ] {
            signature(
                bundle,
                &self.team.requirement(identifier),
                role,
                VerificationPolicy::Launch,
            )?;
            verify_bundle(bundle, identifier, executable)?;
        }
        signature(
            sources.driver(),
            &self.team.requirement("mixrelay-driver-service"),
            ArtifactRole::Driver,
            VerificationPolicy::Launch,
        )?;
        verify_architecture(sources.driver())?;
        Ok(definition)
    }
}

#[derive(Deserialize)]
struct BundleInfo {
    #[serde(rename = "CFBundleIdentifier")]
    identifier: String,
    #[serde(rename = "CFBundleExecutable")]
    executable: String,
}

fn verify_bundle(bundle: &Path, identifier: &str, executable: &str) -> Result<(), ReleaseError> {
    let bytes = read_configuration(&bundle.join("Contents/Info.plist"), 1024 * 1024)?;
    let info: BundleInfo = plist::from_bytes(&bytes)?;
    if info.identifier != identifier || info.executable != executable {
        return Err(ReleaseError::BundleIdentity(bundle.to_path_buf()));
    }
    verify_architecture(&bundle.join("Contents/MacOS").join(executable))
}

fn verify_architecture(path: &Path) -> Result<(), ReleaseError> {
    validate_universal_binary(path).map_err(|source| ReleaseError::Universal {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(any(target_os = "macos", test))]
fn policy_error(
    path: &Path,
    architecture: Architecture,
    violation: PolicyViolation,
) -> ReleaseError {
    let reason = match violation {
        PolicyViolation::ArchitectureSet => "expected exactly arm64 and x86_64 signing slices",
        PolicyViolation::HardenedRuntime => "Hardened Runtime is not enabled",
        PolicyViolation::SecureTimestamp => "secure signing timestamp is missing",
        PolicyViolation::Entitlements => "embedded entitlements differ from the artifact allowlist",
    };
    ReleaseError::SignaturePolicy {
        path: path.to_path_buf(),
        architecture: architecture.name(),
        reason,
    }
}

fn read_configuration(path: &Path, limit: u64) -> Result<Vec<u8>, ReleaseError> {
    let read = || -> io::Result<Vec<u8>> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let file = File::from(descriptor);
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let bytes = read().map_err(|source| ReleaseError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.len() as u64 > limit {
        return Err(ReleaseError::ConfigurationFile(path.to_path_buf()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
