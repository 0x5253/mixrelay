//! Ownership and snapshot identity required to reactivate the previous installation.

use super::{
    ArtifactRole, ReleaseError, ReleasePayloadVerifier, SigningTeam, VerificationPolicy,
    signature::verify_signature,
};
use crate::{FilesystemAdapter, FilesystemLayout, FilesystemSnapshot, OwnedTarget, ServiceAccount};
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Previous owned targets, bound to their filesystem, publisher and service account.
///
/// Native callers must exclude other writers while restoring and reactivating code.
/// Signature verification remains required even when filesystem identity matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedInstallationSnapshot {
    files: FilesystemSnapshot,
    team: SigningTeam,
    account: ServiceAccount,
    entries: BTreeMap<PathBuf, EntryIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EntryIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified: (i64, i64),
}

fn installed_entries(
    layout: &FilesystemLayout,
) -> Result<BTreeMap<PathBuf, EntryIdentity>, ReleaseError> {
    let mut entries = BTreeMap::new();
    for target in OwnedTarget::all() {
        capture_entry(&layout.target_path(target), &mut entries)?;
    }
    Ok(entries)
}

fn capture_entry(
    path: &Path,
    entries: &mut BTreeMap<PathBuf, EntryIdentity>,
) -> Result<(), ReleaseError> {
    let failure = |source| ReleaseError::Io {
        path: path.to_path_buf(),
        source,
    };
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(failure(error)),
    };
    entries.insert(
        path.to_path_buf(),
        EntryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
        },
    );
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(failure)? {
            capture_entry(&entry.map_err(failure)?.path(), entries)?;
        }
    }
    Ok(())
}

impl OwnedInstallationSnapshot {
    fn ensure_entries(&self, layout: &FilesystemLayout) -> Result<(), ReleaseError> {
        let actual = installed_entries(layout)?;
        if actual != self.entries {
            return Err(ReleaseError::SnapshotContents);
        }
        Ok(())
    }
}

impl ReleasePayloadVerifier {
    /// Captures existing owned targets before they are closed, replaced or removed.
    /// # Errors
    /// Returns unsafe paths, invalid ownership or targets that change during inspection.
    pub fn snapshot_owned(
        &self,
        layout: &FilesystemLayout,
    ) -> Result<OwnedInstallationSnapshot, ReleaseError> {
        self.snapshot_owned_with(layout, verify_signature)
    }

    pub(super) fn snapshot_owned_with(
        &self,
        layout: &FilesystemLayout,
        signature: impl FnMut(&Path, &str, ArtifactRole, VerificationPolicy) -> Result<(), ReleaseError>,
    ) -> Result<OwnedInstallationSnapshot, ReleaseError> {
        let filesystem = FilesystemAdapter::new(layout.clone());
        let files = filesystem.snapshot_targets()?;
        let entries = installed_entries(layout)?;
        self.verify_ownership_with(layout, signature)?;
        filesystem.ensure_snapshot(&files)?;
        let snapshot = OwnedInstallationSnapshot {
            files,
            team: self.team.clone(),
            account: self.account.clone(),
            entries,
        };
        snapshot.ensure_entries(layout)?;
        Ok(snapshot)
    }

    /// Verifies exactly the previous owned installation before recovery reactivation.
    ///
    /// Every required target must exist. Every present target must pass ownership
    /// verification and match the snapshot; previous entitlements are unrestricted.
    /// New installation results must use [`Self::verify_installed`] instead.
    /// # Errors
    /// Returns missing dependencies, foreign code, changed targets or snapshot bindings.
    pub fn verify_restored(
        &self,
        layout: &FilesystemLayout,
        required: &[OwnedTarget],
        snapshot: &OwnedInstallationSnapshot,
    ) -> Result<(), ReleaseError> {
        self.verify_restored_with(layout, required, snapshot, verify_signature)
    }

    pub(super) fn verify_restored_with(
        &self,
        layout: &FilesystemLayout,
        required: &[OwnedTarget],
        snapshot: &OwnedInstallationSnapshot,
        signature: impl FnMut(&Path, &str, ArtifactRole, VerificationPolicy) -> Result<(), ReleaseError>,
    ) -> Result<(), ReleaseError> {
        if snapshot.team != self.team || snapshot.account != self.account {
            return Err(ReleaseError::SnapshotIdentity);
        }
        let filesystem = FilesystemAdapter::new(layout.clone());
        filesystem.ensure_snapshot(&snapshot.files)?;
        snapshot.ensure_entries(layout)?;
        Self::require_targets(layout, required)?;
        self.verify_ownership_with(layout, signature)?;
        filesystem.ensure_snapshot(&snapshot.files)?;
        snapshot.ensure_entries(layout)?;
        Ok(())
    }
}
