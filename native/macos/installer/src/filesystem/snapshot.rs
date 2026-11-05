//! Preflight observation of the owned targets, compared again before every mutation.

use std::{fs::Metadata, os::unix::fs::MetadataExt};

use super::{
    FilesystemAdapter,
    error::FilesystemError,
    layout::{FilesystemLayout, TargetKind, staging_name},
    paths::{ensure_no_symlink_components, metadata_if_present},
};
use crate::contract::OwnedTarget;

/// The public presence observed for one owned destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetPresence {
    /// The destination does not exist.
    Absent,
    /// The destination exists with the expected non-symlink kind.
    Present,
}

/// The exact target state captured by filesystem preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemSnapshot {
    pub(super) layout: FilesystemLayout,
    pub(super) targets: [TargetSnapshot; 8],
    pub(super) product_directory: Option<FilesystemIdentity>,
}

impl FilesystemSnapshot {
    /// Returns whether one exact destination was present during preflight.
    #[must_use]
    pub fn presence(&self, target: OwnedTarget) -> TargetPresence {
        self.target(target).presence.public()
    }

    pub(super) fn target(&self, target: OwnedTarget) -> &TargetSnapshot {
        match self
            .targets
            .iter()
            .find(|snapshot| snapshot.target == target)
        {
            Some(snapshot) => snapshot,
            None => unreachable!("OwnedTarget::all always contains every target"),
        }
    }
}

/// Inode identity plus the fields that change when its content is replaced in place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MetadataFingerprint {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanos: i64,
}

/// The device and inode that name one filesystem object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FilesystemIdentity {
    device: u64,
    inode: u64,
}

pub(super) fn fingerprint(metadata: &Metadata) -> MetadataFingerprint {
    MetadataFingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    }
}

pub(super) fn identity(metadata: &Metadata) -> FilesystemIdentity {
    FilesystemIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SnapshotPresence {
    Absent,
    Present(MetadataFingerprint),
}

impl SnapshotPresence {
    pub(super) fn is_present(self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn public(self) -> TargetPresence {
        match self {
            Self::Absent => TargetPresence::Absent,
            Self::Present(_) => TargetPresence::Present,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TargetSnapshot {
    pub(super) target: OwnedTarget,
    pub(super) presence: SnapshotPresence,
}

impl FilesystemAdapter {
    pub(crate) fn snapshot_targets(&self) -> Result<FilesystemSnapshot, FilesystemError> {
        Ok(FilesystemSnapshot {
            layout: self.layout.clone(),
            targets: self.observe_targets()?,
            product_directory: self.observe_product_directory()?,
        })
    }

    /// Fails unless every target and the product folder still match `snapshot`.
    pub(crate) fn ensure_snapshot(
        &self,
        snapshot: &FilesystemSnapshot,
    ) -> Result<(), FilesystemError> {
        if snapshot.layout != self.layout {
            return Err(FilesystemError::ReceiptMismatch);
        }
        if self.observe_product_directory()? != snapshot.product_directory {
            return Err(FilesystemError::CreatedDirectoryChanged {
                path: self.product_directory_path(),
            });
        }
        for target in OwnedTarget::all() {
            self.ensure_unchanged(snapshot, target)?;
        }
        Ok(())
    }

    pub(super) fn ensure_unchanged(
        &self,
        snapshot: &FilesystemSnapshot,
        target: OwnedTarget,
    ) -> Result<TargetSnapshot, FilesystemError> {
        let observed = self.observe_target(target)?;
        if &observed == snapshot.target(target) {
            Ok(observed)
        } else {
            Err(FilesystemError::ConcurrentTargetChange { target })
        }
    }

    fn observe_targets(&self) -> Result<[TargetSnapshot; 8], FilesystemError> {
        let mut targets = OwnedTarget::all().map(|target| TargetSnapshot {
            target,
            presence: SnapshotPresence::Absent,
        });
        for snapshot in &mut targets {
            *snapshot = self.observe_target(snapshot.target)?;
        }
        Ok(targets)
    }

    fn observe_target(&self, target: OwnedTarget) -> Result<TargetSnapshot, FilesystemError> {
        let path = self.layout.target_path(target);
        ensure_no_symlink_components(&path, true)?;
        let Some(metadata) = metadata_if_present(&path, "inspect")? else {
            return Ok(TargetSnapshot {
                target,
                presence: SnapshotPresence::Absent,
            });
        };
        if metadata.file_type().is_symlink() {
            return Err(FilesystemError::UnsafePath {
                path,
                reason: "owned target is a symlink",
            });
        }
        if !TargetKind::of(target).matches(&metadata) {
            return Err(FilesystemError::UnexpectedKind {
                kind: staging_name(target),
                path,
            });
        }
        Ok(TargetSnapshot {
            target,
            presence: SnapshotPresence::Present(fingerprint(&metadata)),
        })
    }
}
