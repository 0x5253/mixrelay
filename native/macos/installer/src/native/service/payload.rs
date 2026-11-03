//! Validates installed payload identity and writes the account-bound launchd plist.

use super::{super::trust::validate_owned_target, NativeServiceLifecycle};
use crate::{
    FilesystemLayout, FilesystemStaged, HAL_PATH, LaunchdConfiguration, NativeError, OperationKind,
    OwnedTarget,
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

impl NativeServiceLifecycle {
    pub(super) fn verify_installed_payload(
        &self,
        layout: &FilesystemLayout,
    ) -> Result<Option<LaunchdConfiguration>, NativeError> {
        let path_for =
            |target: OwnedTarget| layout.root().join(target.path().trim_start_matches('/'));
        let present_targets = present_targets(layout)?;
        if present_targets.is_empty() {
            return Ok(None);
        }
        let core_present = present_targets.iter().any(|target| {
            matches!(
                target,
                OwnedTarget::App | OwnedTarget::Driver | OwnedTarget::Hal | OwnedTarget::Launchd
            )
        });
        for target in &present_targets {
            let mode = match target {
                OwnedTarget::Launchd
                | OwnedTarget::RemovalLaunchd
                | OwnedTarget::RuntimeLaunchd => 0o644,
                _ => 0o755,
            };
            validate_owned_target(&path_for(*target), mode)?;
        }
        if self.operation == OperationKind::Remove {
            crate::removal::verify_remaining_remover(&self.team).map_err(NativeError::UserData)?;
            return self
                .verifier
                .verify_ownership(layout)
                .map_err(NativeError::from);
        }
        if present_targets.contains(&OwnedTarget::RuntimeEngine)
            != present_targets.contains(&OwnedTarget::RuntimeLaunchd)
        {
            return Err(NativeError::Trust {
                path: layout.root().to_path_buf(),
                reason: "runtime installation is partial",
            });
        }
        if !core_present {
            self.verifier.verify_ownership(layout)?;
            return Ok(None);
        }
        // Verify every present target separately. A missing component may be
        // repaired, but foreign code never becomes ours merely because it
        // occupies an expected path.
        self.verifier
            .verify_ownership(layout)
            .map_err(NativeError::from)
    }

    pub(super) fn write_bound_plist(
        staged: &FilesystemStaged,
        definition: &LaunchdConfiguration,
    ) -> Result<(), NativeError> {
        let path = staged.payload_path(OwnedTarget::Launchd);
        let bytes = definition.to_xml()?;
        write_private_plist(&path, &bytes)
    }
}

fn present_targets(layout: &FilesystemLayout) -> Result<Vec<OwnedTarget>, NativeError> {
    let path_for = |target: OwnedTarget| layout.root().join(target.path().trim_start_matches('/'));
    let mut present_targets = Vec::new();
    for target in OwnedTarget::all() {
        let path = path_for(target);
        match fs::symlink_metadata(&path) {
            Ok(_) => present_targets.push(target),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(NativeError::Io {
                    operation: "inspect owned payload",
                    path,
                    source,
                });
            }
        }
    }
    Ok(present_targets)
}

fn write_private_plist(path: &std::path::Path, bytes: &[u8]) -> Result<(), NativeError> {
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|source| NativeError::Io {
            operation: "open private launchd payload",
            path: path.to_path_buf(),
            source,
        })?;
    let metadata = file.metadata().map_err(|source| NativeError::Io {
        operation: "inspect private launchd payload",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != 0 {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "private launchd payload is not a private root-owned regular inode",
        });
    }
    file.set_len(0).map_err(|source| NativeError::Io {
        operation: "truncate private launchd payload",
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes).map_err(|source| NativeError::Io {
        operation: "write account-bound launchd payload",
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| NativeError::Io {
        operation: "sync account-bound launchd payload",
        path: path.to_path_buf(),
        source,
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).map_err(|source| NativeError::Io {
        operation: "set account-bound launchd payload mode",
        path: path.to_path_buf(),
        source,
    })
}

impl NativeServiceLifecycle {
    pub(super) fn write_runtime_plist(&self, staged: &FilesystemStaged) -> Result<(), NativeError> {
        let path = staged.payload_path(OwnedTarget::RuntimeLaunchd);
        if path.exists() {
            let definition = crate::runtime_definition::for_account(self.account.account())
                .map_err(NativeError::UserData)?;
            let mut bytes = Vec::new();
            definition
                .to_writer_xml(&mut bytes)
                .map_err(|error| NativeError::UserData(error.into()))?;
            write_private_plist(&path, &bytes)?;
        }
        Ok(())
    }
}

pub(super) fn installed_hal_version() -> Result<String, NativeError> {
    let path = PathBuf::from(HAL_PATH).join("Contents/Info.plist");
    let bytes = read_bounded_file(&path)?;
    hal_release_version(&bytes)
}

fn hal_release_version(bytes: &[u8]) -> Result<String, NativeError> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|error| NativeError::Activation(format!("invalid HAL version plist: {error}")))?;
    value
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("CFBundleShortVersionString"))
        .and_then(plist::Value::as_string)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| NativeError::Activation("signed HAL release version is absent".to_owned()))
}

pub(super) fn read_bounded_file(path: &std::path::Path) -> Result<Vec<u8>, NativeError> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|source| NativeError::Io {
        operation: "read bounded installed metadata",
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| NativeError::Io {
        operation: "inspect bounded installed metadata",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "installed metadata is not a bounded regular file",
        });
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| NativeError::Io {
            operation: "read bounded installed metadata",
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > 16 * 1024 {
        return Err(NativeError::Trust {
            path: path.to_path_buf(),
            reason: "installed metadata exceeds its size limit",
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hal_release_version_requires_the_committed_bundle_version() {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "CFBundleShortVersionString".to_owned(),
            plist::Value::String("0.1.0".to_owned()),
        );
        let mut bytes = Vec::new();
        plist::Value::Dictionary(dictionary)
            .to_writer_xml(&mut bytes)
            .unwrap();
        assert_eq!(hal_release_version(&bytes).unwrap(), "0.1.0");
        assert!(hal_release_version(b"<plist version=\"1.0\"><dict/></plist>").is_err());
        assert!(hal_release_version(b"invalid").is_err());
    }
}
