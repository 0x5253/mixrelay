//! Verifies signature integrity and publisher identity before applying the selected policy.

#[cfg(target_os = "macos")]
use super::policy::verify_slices;
use super::{
    ReleaseError,
    policy::{ArtifactRole, VerificationPolicy},
};
use std::path::Path;

#[cfg(not(target_os = "macos"))]
pub(super) fn verify_signature(
    _path: &Path,
    _requirement: &str,
    _role: ArtifactRole,
    _policy: VerificationPolicy,
) -> Result<(), ReleaseError> {
    Err(ReleaseError::UnsupportedPlatform)
}

#[cfg(target_os = "macos")]
pub(super) fn verify_signature(
    path: &Path,
    requirement: &str,
    role: ArtifactRole,
    policy: VerificationPolicy,
) -> Result<(), ReleaseError> {
    use std::{
        process::{Command, Stdio},
        time::Duration,
    };
    let literal_requirement = literal_requirement(requirement);
    let mut command = Command::new("/usr/bin/codesign");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C")
        .args([
            "--verify",
            "--deep",
            "--strict",
            "--all-architectures",
            "--test-requirement",
            &literal_requirement,
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    run_checked(&mut command, path, Duration::from_secs(30))?;
    verify_signature_metadata(path, role, policy)
}

#[cfg(target_os = "macos")]
fn literal_requirement(requirement: &str) -> String {
    // Without '=' codesign interprets a requirement as a filename, not source.
    format!("={requirement}")
}

#[cfg(target_os = "macos")]
fn verify_signature_metadata(
    path: &Path,
    role: ArtifactRole,
    policy: VerificationPolicy,
) -> Result<(), ReleaseError> {
    let metadata_path = match role {
        ArtifactRole::Application => path.join("Contents/MacOS/mixrelay-desktop"),
        ArtifactRole::Hal => path.join("Contents/MacOS/MixRelayHAL"),
        ArtifactRole::Uninstaller => path.join("Contents/MacOS/mixrelay-driver-uninstaller"),
        ArtifactRole::Driver | ArtifactRole::Engine | ArtifactRole::Installer => path.to_path_buf(),
    };
    let slices = super::security_metadata::inspect_slices(&metadata_path, role, policy)?;
    verify_slices(role, policy, &slices)
        .map_err(|(architecture, violation)| super::policy_error(path, architecture, violation))
}

#[cfg(target_os = "macos")]
fn run_checked(
    command: &mut std::process::Command,
    path: &Path,
    timeout: std::time::Duration,
) -> Result<(), ReleaseError> {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let mut owner = ChildOwner(command.spawn().map_err(|source| ReleaseError::Io {
        path: path.to_path_buf(),
        source,
    })?);
    let deadline = Instant::now() + timeout;
    wait_checked(
        &mut owner.0,
        path,
        || Instant::now() >= deadline,
        || thread::sleep(Duration::from_millis(10)),
    )
}

/// The parts of a spawned verifier that [`wait_checked`] drives.
#[cfg(target_os = "macos")]
trait VerifierProcess {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
    fn kill_and_reap(&mut self) -> std::io::Result<()>;
}

#[cfg(target_os = "macos")]
impl VerifierProcess for std::process::Child {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        std::process::Child::try_wait(self)
    }
    fn kill_and_reap(&mut self) -> std::io::Result<()> {
        self.kill()?;
        self.wait().map(|_| ())
    }
}

/// Polls the verifier until it exits, or kills and reaps it once `expired` reports the
/// deadline, calling `pause` between polls.
#[cfg(target_os = "macos")]
fn wait_checked(
    process: &mut impl VerifierProcess,
    path: &Path,
    mut expired: impl FnMut() -> bool,
    mut pause: impl FnMut(),
) -> Result<(), ReleaseError> {
    let io_error = |source| ReleaseError::Io {
        path: path.to_path_buf(),
        source,
    };
    loop {
        if let Some(status) = process.try_wait().map_err(io_error)? {
            return if status.success() {
                Ok(())
            } else {
                Err(ReleaseError::Signature {
                    path: path.to_path_buf(),
                    status: status.to_string(),
                })
            };
        }
        if expired() {
            process.kill_and_reap().map_err(io_error)?;
            return Err(ReleaseError::Timeout(path.to_path_buf()));
        }
        pause();
    }
}

#[cfg(target_os = "macos")]
struct ChildOwner(std::process::Child);

#[cfg(target_os = "macos")]
impl Drop for ChildOwner {
    fn drop(&mut self) {
        // Every early error/panic owns and reaps this exact child; no background
        // verifier or output-collection thread survives the transaction boundary.
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    use std::{os::unix::process::ExitStatusExt, process::ExitStatus};

    /// A verifier that exits with `status` after `running` polls, if ever.
    struct FakeProcess {
        running: Option<usize>,
        status: ExitStatus,
        killed: bool,
    }

    impl VerifierProcess for FakeProcess {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            Ok(match &mut self.running {
                Some(0) => Some(self.status),
                Some(remaining) => {
                    *remaining -= 1;
                    None
                }
                None => None,
            })
        }
        fn kill_and_reap(&mut self) -> std::io::Result<()> {
            self.killed = true;
            Ok(())
        }
    }

    fn wait(process: &mut FakeProcess, deadline_after: usize) -> Result<(), ReleaseError> {
        let mut polls = 0;
        wait_checked(
            process,
            Path::new("verifier"),
            || {
                polls += 1;
                polls > deadline_after
            },
            || {},
        )
    }

    #[test]
    fn requirement_is_compiled_as_source_not_interpreted_as_a_path() {
        assert_eq!(literal_requirement("anchor apple"), "=anchor apple");
    }

    #[test]
    fn verifier_success_failure_and_deadline_are_distinct() {
        let mut success = FakeProcess {
            running: Some(2),
            status: ExitStatus::from_raw(0),
            killed: false,
        };
        wait(&mut success, 5).unwrap();
        assert!(!success.killed);
        let mut failure = FakeProcess {
            running: Some(0),
            status: ExitStatus::from_raw(1 << 8),
            killed: false,
        };
        assert!(matches!(
            wait(&mut failure, 5),
            Err(ReleaseError::Signature { .. })
        ));
        let mut hung = FakeProcess {
            running: None,
            status: ExitStatus::from_raw(0),
            killed: false,
        };
        assert!(matches!(wait(&mut hung, 3), Err(ReleaseError::Timeout(_))));
        assert!(hung.killed);
    }
}
