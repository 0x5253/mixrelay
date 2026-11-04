//! What a commit does to each owned target, decided from the request and preflight alone.

use crate::{OperationKind, mutation_journal::FilesystemStageState};

/// The steps one target takes during commit, in this order.
///
/// Targets are committed in `OwnedTarget::all()` order; rollback undoes the recorded
/// steps in reverse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TargetSteps {
    /// Create the destination's missing parent directories.
    pub(super) prepare_parent: bool,
    /// Move the existing target into private backups.
    pub(super) back_up: bool,
    /// Move the staged payload into the destination.
    pub(super) install: bool,
}

impl TargetSteps {
    pub(super) fn plan(operation: OperationKind, present: bool) -> Self {
        let install = operation == OperationKind::Install;
        Self {
            prepare_parent: present || install,
            back_up: present,
            install,
        }
    }
}

/// Only a committed removal leaves the product folder to clean; rollback restores it.
pub(super) fn cleans_product_directory(
    state: FilesystemStageState,
    operation: OperationKind,
) -> bool {
    state == FilesystemStageState::Committed && operation == OperationKind::Remove
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(prepare_parent: bool, back_up: bool, install: bool) -> TargetSteps {
        TargetSteps {
            prepare_parent,
            back_up,
            install,
        }
    }

    #[test]
    fn install_backs_up_an_existing_target_before_replacing_it() {
        assert_eq!(
            TargetSteps::plan(OperationKind::Install, true),
            steps(true, true, true)
        );
        assert_eq!(
            TargetSteps::plan(OperationKind::Install, false),
            steps(true, false, true)
        );
    }

    #[test]
    fn removal_only_backs_up_present_targets_and_never_creates_parents_for_absent_ones() {
        assert_eq!(
            TargetSteps::plan(OperationKind::Remove, true),
            steps(true, true, false)
        );
        assert_eq!(
            TargetSteps::plan(OperationKind::Remove, false),
            steps(false, false, false)
        );
    }

    #[test]
    fn product_folder_cleanup_follows_only_a_committed_removal() {
        use FilesystemStageState::{
            Cleaned, CommitFailed, Committed, RollbackFailed, RolledBack, Uncommitted,
        };
        assert!(cleans_product_directory(Committed, OperationKind::Remove));
        assert!(!cleans_product_directory(Committed, OperationKind::Install));
        for state in [
            Uncommitted,
            CommitFailed,
            RolledBack,
            RollbackFailed,
            Cleaned,
        ] {
            assert!(!cleans_product_directory(state, OperationKind::Remove));
        }
    }
}
