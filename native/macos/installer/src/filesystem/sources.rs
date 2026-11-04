//! Install source validation: shape, executability and separation from every destination.

use std::{
    fs::Metadata,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::{
    FilesystemAdapter,
    error::FilesystemError,
    layout::{TargetKind, staging_name},
    paths::{ensure_no_symlink_components, is_within, metadata, validate_absolute_path},
    tree::validate_tree,
};
use crate::{InstallSources, contract::OwnedTarget};

/// The order in which sources are checked, which fixes the first error reported.
const VALIDATION_ORDER: [OwnedTarget; 8] = [
    OwnedTarget::App,
    OwnedTarget::Hal,
    OwnedTarget::Driver,
    OwnedTarget::Launchd,
    OwnedTarget::Uninstaller,
    OwnedTarget::RemovalLaunchd,
    OwnedTarget::RuntimeEngine,
    OwnedTarget::RuntimeLaunchd,
];

/// The source artifact that installs `target`.
pub(super) fn source_path(sources: &InstallSources, target: OwnedTarget) -> &Path {
    match target {
        OwnedTarget::App => sources.app(),
        OwnedTarget::Driver => sources.driver(),
        OwnedTarget::Hal => sources.hal(),
        OwnedTarget::Launchd => sources.launchd(),
        OwnedTarget::Uninstaller => sources.remover(),
        OwnedTarget::RemovalLaunchd => sources.removal_launchd(),
        OwnedTarget::RuntimeEngine => sources.runtime_engine(),
        OwnedTarget::RuntimeLaunchd => sources.runtime_launchd(),
    }
}

/// The bundle executable that must exist for a directory target.
fn bundle_executable(target: OwnedTarget) -> &'static str {
    match target {
        OwnedTarget::App => "Contents/MacOS/mixrelay-desktop",
        OwnedTarget::Uninstaller => "Contents/MacOS/mixrelay-driver-uninstaller",
        _ => "Contents/MacOS/MixRelayHAL",
    }
}

impl FilesystemAdapter {
    pub(super) fn validate_sources(&self, sources: &InstallSources) -> Result<(), FilesystemError> {
        let paths = VALIDATION_ORDER.map(|target| (target, source_path(sources, target)));
        ensure_sources_disjoint(&paths.map(|(_, path)| path))?;
        for (target, path) in paths {
            validate_source(target, path)?;
            self.ensure_source_outside_destinations(path)?;
        }
        Ok(())
    }

    fn ensure_source_outside_destinations(&self, source: &Path) -> Result<(), FilesystemError> {
        let staging = self.layout.staging_root();
        let overlaps = |other: &Path| is_within(source, other) || is_within(other, source);
        if overlaps(staging)
            || OwnedTarget::all()
                .iter()
                .any(|target| overlaps(&self.layout.target_path(*target)))
        {
            return Err(FilesystemError::UnsafePath {
                path: source.to_path_buf(),
                reason: "source overlaps an owned destination or private staging",
            });
        }
        Ok(())
    }
}

/// No source may contain another, or one copy would include a second artifact.
fn ensure_sources_disjoint(paths: &[&Path]) -> Result<(), FilesystemError> {
    for (index, path) in paths.iter().enumerate() {
        for other in &paths[index + 1..] {
            if is_within(path, other) || is_within(other, path) {
                return Err(FilesystemError::UnsafePath {
                    path: path.to_path_buf(),
                    reason: "source artifacts overlap",
                });
            }
        }
    }
    Ok(())
}

fn validate_source(target: OwnedTarget, path: &Path) -> Result<(), FilesystemError> {
    let kind = staging_name(target);
    validate_absolute_path(path)?;
    ensure_no_symlink_components(path, false)?;
    let metadata = metadata(path, kind)?;
    let expected = TargetKind::of(target);
    if !expected.matches(&metadata) {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: path.to_path_buf(),
            reason: "unexpected inode kind",
        });
    }
    match expected {
        TargetKind::File => validate_file_source(target, path, &metadata),
        TargetKind::Directory => validate_bundle_source(target, path),
    }
}

fn validate_file_source(
    target: OwnedTarget,
    path: &Path,
    metadata: &Metadata,
) -> Result<(), FilesystemError> {
    let kind = staging_name(target);
    if metadata.len() == 0 {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: path.to_path_buf(),
            reason: "file is empty",
        });
    }
    let executable = matches!(target, OwnedTarget::Driver | OwnedTarget::RuntimeEngine);
    if executable && metadata.permissions().mode() & 0o111 == 0 {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: path.to_path_buf(),
            reason: "Driver is not executable",
        });
    }
    Ok(())
}

fn validate_bundle_source(target: OwnedTarget, path: &Path) -> Result<(), FilesystemError> {
    let kind = staging_name(target);
    let info = path.join("Contents/Info.plist");
    if !metadata(&info, kind)?.is_file() {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: info,
            reason: "expected regular file",
        });
    }
    let executable: PathBuf = path.join(bundle_executable(target));
    let executable_metadata = metadata(&executable, kind)?;
    if !executable_metadata.is_file()
        || executable_metadata.len() == 0
        || executable_metadata.permissions().mode() & 0o111 == 0
    {
        return Err(FilesystemError::InvalidSource {
            kind,
            path: executable,
            reason: "bundle executable is missing, empty, or not executable",
        });
    }
    validate_tree(path)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use mixrelay_test_support::FixtureDir;

    use super::*;

    #[test]
    fn nested_or_identical_sources_are_rejected_as_overlapping() {
        let (a, b, c) = (Path::new("/s/a"), Path::new("/s/b"), Path::new("/s/a/c"));
        assert!(ensure_sources_disjoint(&[a, b]).is_ok());
        for paths in [[a, c], [c, a], [a, a]] {
            assert!(matches!(
                ensure_sources_disjoint(&paths),
                Err(FilesystemError::UnsafePath {
                    reason: "source artifacts overlap",
                    ..
                })
            ));
        }
    }

    #[test]
    fn service_executables_must_be_nonempty_and_executable_while_plists_need_only_content() {
        let root = FixtureDir::new("sources-files").expect("fixture");
        let path = root.join("artifact");
        fs::write(&path, b"content").expect("artifact");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
        for target in [OwnedTarget::Driver, OwnedTarget::RuntimeEngine] {
            assert!(matches!(
                validate_source(target, &path),
                Err(FilesystemError::InvalidSource {
                    reason: "Driver is not executable",
                    ..
                })
            ));
        }
        for target in [
            OwnedTarget::Launchd,
            OwnedTarget::RemovalLaunchd,
            OwnedTarget::RuntimeLaunchd,
        ] {
            assert!(validate_source(target, &path).is_ok());
        }
        fs::write(&path, b"").expect("empty");
        assert!(matches!(
            validate_source(OwnedTarget::Launchd, &path),
            Err(FilesystemError::InvalidSource {
                reason: "file is empty",
                ..
            })
        ));
    }

    #[test]
    fn bundles_need_their_own_executable_and_no_symlinks() {
        let root = FixtureDir::new("sources-bundles").expect("fixture");
        let bundle = root.join("Bundle.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).expect("bundle");
        fs::write(bundle.join("Contents/Info.plist"), b"info").expect("info");
        let executable = bundle.join(bundle_executable(OwnedTarget::App));
        fs::write(&executable, b"app").expect("executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("mode");
        assert!(validate_source(OwnedTarget::App, &bundle).is_ok());
        assert!(matches!(
            validate_source(OwnedTarget::Uninstaller, &bundle),
            Err(FilesystemError::Missing { .. })
        ));
        std::os::unix::fs::symlink("/etc/hosts", bundle.join("Contents/link")).expect("symlink");
        assert!(matches!(
            validate_source(OwnedTarget::App, &bundle),
            Err(FilesystemError::UnsafePath { .. })
        ));
    }
}
