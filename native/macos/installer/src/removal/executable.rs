//! Keeps the signed helper available until its removal reply is acknowledged.

use super::{HELPER, validate_root_file, verification_account};
use crate::{ReleasePayloadVerifier, SigningTeam};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

pub(super) struct RemovalExecutable {
    directory: PathBuf,
    pub(super) executable: PathBuf,
    identity: (u64, u64),
}

impl RemovalExecutable {
    pub(super) fn from_current_process(team: &SigningTeam) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let directory = executable
            .parent()
            .context("missing helper staging directory")?;
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let suffix = name
            .strip_prefix("io.mixrelay.driver.remove.")
            .unwrap_or_default();
        ensure!(
            directory.parent() == Some(Path::new("/private/tmp"))
                && suffix.len() == 6
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
                && executable.file_name() == Some(std::ffi::OsStr::new("mixrelay-macos-installer")),
            "removal server must use its verified private executable"
        );
        let temporary = fs::symlink_metadata("/private/tmp")?;
        ensure!(
            temporary.is_dir() && temporary.uid() == 0 && temporary.mode() & 0o7777 == 0o1777,
            "untrusted temporary parent"
        );
        let metadata = fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o755,
            "untrusted removal staging directory"
        );
        validate_root_file(&executable, 0o755)?;
        ReleasePayloadVerifier::new(team.clone(), verification_account()?)
            .verify_installer_executable(&executable)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            executable,
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    pub(super) fn remove_private_copy(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.directory)?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == 0
                && (metadata.dev(), metadata.ino()) == self.identity,
            "removal staging identity changed; retaining it"
        );
        fs::remove_dir_all(&self.directory).context("could not clean private removal executable")
    }

    pub(super) fn create_private_copy(team: &SigningTeam) -> Result<Self> {
        let temporary = fs::symlink_metadata("/private/tmp")?;
        ensure!(
            temporary.is_dir() && temporary.uid() == 0 && temporary.mode() & 0o7777 == 0o1777,
            "untrusted temporary parent"
        );
        let output = Command::new("/usr/bin/mktemp")
            .args(["-d", "/private/tmp/io.mixrelay.driver.remove.XXXXXX"])
            .output()?;
        ensure!(
            output.status.success(),
            "could not stage fixed removal executable"
        );
        let directory = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        let metadata = fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && metadata.uid() == 0,
            "invalid removal staging directory"
        );
        let executable = directory.join("mixrelay-macos-installer");
        let stage = Self {
            directory,
            executable,
            identity: (metadata.dev(), metadata.ino()),
        };
        fs::set_permissions(&stage.directory, fs::Permissions::from_mode(0o755))?;
        fs::copy(HELPER, &stage.executable)?;
        fs::set_permissions(&stage.executable, fs::Permissions::from_mode(0o755))?;
        validate_root_file(&stage.executable, 0o755)?;
        ReleasePayloadVerifier::new(team.clone(), verification_account()?)
            .verify_installer_executable(&stage.executable)?;
        Ok(stage)
    }
}

impl Drop for RemovalExecutable {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.directory)
            && metadata.is_dir()
            && metadata.uid() == 0
            && (metadata.dev(), metadata.ino()) == self.identity
        {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}
