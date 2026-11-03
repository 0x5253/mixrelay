//! Explicit per-user cleanup after a successful distribution removal.

use std::{
    ffi::CStr,
    io::{self, Read, Write},
    os::unix::process::CommandExt as _,
    path::Path,
    process::{Child, Command, Stdio},
};

use anyhow::Context as _;
use rustix::{
    fd::OwnedFd,
    fs::{AtFlags, Mode, OFlags, open, openat, unlinkat},
    io::Errno,
};

use super::ResolvedServiceAccount;

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

const COMMIT_TOKEN: u8 = b'C';

/// A prepared account-owned cleanup that remains inert until removal succeeds.
pub(super) struct PendingCleanup {
    child: Child,
}

impl PendingCleanup {
    fn spawn(command: &mut Command) -> anyhow::Result<Self> {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context("prepare user-data cleanup as the installed service account")?;
        Ok(Self { child })
    }

    /// Releases the prepared cleanup and waits for its completion.
    /// # Errors
    /// Returns a failed commit write, child wait, or cleanup exit status.
    pub(super) fn commit(mut self) -> anyhow::Result<()> {
        let mut input = self
            .child
            .stdin
            .take()
            .context("cleanup commit pipe is absent")?;
        input
            .write_all(&[COMMIT_TOKEN])
            .context("commit Driver data cleanup")?;
        drop(input);
        let status = self.child.wait().context("reap Driver data cleanup")?;
        anyhow::ensure!(status.success(), "Driver data cleanup failed ({status})");
        Ok(())
    }
}

impl Drop for PendingCleanup {
    fn drop(&mut self) {
        // EOF cancels an uncommitted cleanup before rollback restores the remover app.
        drop(self.child.stdin.take());
        if let Err(error) = self.child.wait() {
            tracing::error!("could not reap Driver cleanup child: {error}");
        }
    }
}

/// Loads the current installer executable before its remover app bundle is removed.
/// # Errors
/// Returns if the executable cannot be resolved or the account-owned child cannot start.
pub(super) fn prepare_for_account(
    account: &ResolvedServiceAccount,
    delete_settings: bool,
) -> anyhow::Result<PendingCleanup> {
    // Setting uid also clears supplementary groups. Cleanup never traverses a
    // user-writable tree with the Installer's root privileges.
    PendingCleanup::spawn(
        Command::new(std::env::current_exe()?)
            .arg("cleanup-user-data")
            .arg(account.account().home())
            .arg(if delete_settings {
                "--delete-user-data"
            } else {
                "--preserve-user-data"
            })
            .arg("--wait-for-commit")
            .env_clear()
            .env("HOME", account.account().home())
            .gid(account.gid())
            .uid(account.uid()),
    )
}

/// Removes the Driver device settings when the user selected their deletion.
///
/// Graph App caches, graphs and recordings are never removed.
/// `home` is the OS-resolved installation home; unchecked removal preserves settings.
/// When `wait_for_commit` is true, one commit byte must arrive on stdin; EOF cancels cleanup.
/// # Errors
/// Returns if called as root, the token is invalid, a parent is a symlink, or removal fails.
pub fn cleanup_user_data(
    home: &Path,
    delete_settings: bool,
    wait_for_commit: bool,
) -> anyhow::Result<()> {
    // SAFETY: geteuid is a read-only identity query with no pointer arguments.
    anyhow::ensure!(
        unsafe { libc::geteuid() } != 0,
        "cleanup must run without root privileges"
    );
    if wait_for_commit && !read_commit(&mut io::stdin().lock())? {
        return Ok(());
    }
    cleanup_files(home, delete_settings).context("clean MixRelay data")
}

fn read_commit(input: &mut impl Read) -> anyhow::Result<bool> {
    let mut token = [0];
    match input.read_exact(&mut token) {
        Ok(()) => {
            anyhow::ensure!(token[0] == COMMIT_TOKEN, "invalid cleanup commit token");
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error).context("read cleanup commit token"),
    }
}

fn cleanup_files(home: &Path, delete_settings: bool) -> anyhow::Result<()> {
    if !delete_settings {
        return Ok(());
    }
    let home = open(home, DIRECTORY_FLAGS, Mode::empty())?;
    let Some(parent) = open_directory(&home, "Library/Application Support/io.mixrelay.driver")?
    else {
        return Ok(());
    };
    remove_file(&parent, c"drivers.json")?;
    Ok(())
}

fn remove_file(parent: &OwnedFd, name: &CStr) -> Result<(), Errno> {
    match unlinkat(parent, name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(error) => Err(error),
    }
}

fn open_directory(parent: &OwnedFd, path: &str) -> Result<Option<OwnedFd>, Errno> {
    let mut directory = openat(parent, ".", DIRECTORY_FLAGS, Mode::empty())?;
    for component in path.split('/') {
        match openat(&directory, component, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(next) => directory = next,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(directory))
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor, os::unix::fs::symlink, path::PathBuf, process::Command};

    use mixrelay_test_support::FixtureDir;

    fn settings_fixture(label: &str) -> FixtureDir {
        let home = FixtureDir::new(label).expect("fixture");
        let settings = home.join("Library/Application Support/io.mixrelay.driver");
        fs::create_dir_all(&settings).expect("settings directory");
        fs::write(settings.join("drivers.json"), b"devices").expect("driver settings");
        home
    }

    fn prepared_fixture(
        executable: &std::path::Path,
        home: &std::path::Path,
    ) -> super::PendingCleanup {
        let pending = super::PendingCleanup::spawn(
            Command::new(executable)
                .args([
                    "--exact",
                    "native::user_data::tests::cleanup_child_fixture",
                    "--nocapture",
                ])
                .env_clear()
                .env("MIXRELAY_CLEANUP_FIXTURE_HOME", home),
        )
        .expect("prepared cleanup fixture");
        // Wait for actual child startup before testing unlink of its executable.
        // spawn() alone can return while dyld is still opening that path.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !home.join("cleanup-test-ready").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "cleanup child did not reach commit wait"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        pending
    }

    #[test]
    fn cleanup_child_fixture() {
        let Some(home) = std::env::var_os("MIXRELAY_CLEANUP_FIXTURE_HOME") else {
            return;
        };
        fs::write(PathBuf::from(&home).join("cleanup-test-ready"), b"ready")
            .expect("startup signal");
        if super::read_commit(&mut std::io::stdin().lock()).expect("commit read") {
            super::cleanup_files(&PathBuf::from(home), true).expect("committed cleanup");
        }
    }

    #[test]
    fn commit_token_is_bounded_and_eof_cancels() {
        assert!(!super::read_commit(&mut Cursor::new([])).expect("EOF"));
        assert!(super::read_commit(&mut Cursor::new([super::COMMIT_TOKEN])).expect("commit"));
        assert!(super::read_commit(&mut Cursor::new([b'X'])).is_err());
    }

    #[test]
    fn prepared_child_commits_after_its_original_executable_path_is_removed() {
        let home = settings_fixture("driver-cleanup-prepared");
        let executable = home.join("cleanup-test-executable");
        fs::hard_link(
            std::env::current_exe().expect("test executable"),
            &executable,
        )
        .expect("fixture executable link");
        let pending = prepared_fixture(&executable, &home);
        fs::remove_file(&executable).expect("remove original child executable path");
        pending.commit().expect("commit loaded executable");
        assert!(
            !home
                .join("Library/Application Support/io.mixrelay.driver/drivers.json")
                .exists()
        );
    }

    #[test]
    fn dropping_prepared_child_cancels_and_reaps_without_deletion() {
        let home = settings_fixture("driver-cleanup-cancel");
        let pending = prepared_fixture(&std::env::current_exe().expect("test executable"), &home);
        drop(pending);
        assert_eq!(
            fs::read(home.join("Library/Application Support/io.mixrelay.driver/drivers.json"))
                .expect("preserved settings"),
            b"devices"
        );
    }

    #[test]
    fn selected_removal_deletes_only_driver_settings_and_is_idempotent() {
        let home = FixtureDir::new("driver-uninstall-user-data").expect("fixture");
        let settings = home.join("Library/Application Support/io.mixrelay.driver");
        fs::create_dir_all(&settings).expect("settings directory");
        fs::write(settings.join("drivers.json"), b"devices").expect("driver settings");
        for name in ["graphs.json", "audio-session.json", "recording.wav"] {
            fs::write(settings.join(name), b"retained").expect("retained data");
        }
        super::cleanup_files(&home, true).expect("cleanup");
        assert!(!settings.join("drivers.json").exists());
        for name in ["graphs.json", "audio-session.json", "recording.wav"] {
            assert_eq!(
                fs::read(settings.join(name)).expect("retained data"),
                b"retained"
            );
        }
        super::cleanup_files(&home, true).expect("idempotent cleanup");
    }

    #[test]
    fn unchecked_removal_preserves_driver_settings() {
        let home = FixtureDir::new("driver-uninstall-preserve").expect("fixture");
        let settings = home.join("Library/Application Support/io.mixrelay.driver");
        fs::create_dir_all(&settings).expect("settings directory");
        fs::write(settings.join("drivers.json"), b"devices").expect("settings");
        super::cleanup_files(&home, false).expect("unchecked removal");
        assert_eq!(
            fs::read(settings.join("drivers.json")).expect("settings"),
            b"devices"
        );
    }

    #[test]
    fn cleanup_rejects_symlink_ancestors_and_never_recurses_into_a_settings_filename() {
        let home = FixtureDir::new("driver-uninstall-symlink").expect("fixture");
        let external = FixtureDir::new("driver-uninstall-external").expect("fixture");
        fs::write(external.join("drivers.json"), b"external").expect("external settings");
        fs::create_dir_all(home.join("Library/Application Support")).expect("parents");
        symlink(
            &external,
            home.join("Library/Application Support/io.mixrelay.driver"),
        )
        .expect("symlink");
        assert!(super::cleanup_files(&home, true).is_err());
        assert_eq!(
            fs::read(external.join("drivers.json")).expect("external"),
            b"external"
        );

        let home = FixtureDir::new("driver-uninstall-occupied").expect("fixture");
        let occupied = home.join("Library/Application Support/io.mixrelay.driver/drivers.json");
        fs::create_dir_all(&occupied).expect("occupied filename");
        fs::write(occupied.join("recording.wav"), b"retained").expect("recording");
        assert!(super::cleanup_files(&home, true).is_err());
        assert!(occupied.join("recording.wav").exists());
    }
}
