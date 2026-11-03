//! Successful mutations are undone in reverse order through the installation store.
use crate::OwnedTarget;

/// The mutation state held by a staged filesystem receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemStageState {
    /// No commit or rollback has attempted to mutate owned destinations.
    Uncommitted,
    /// All owned destination mutations completed; registration is an external boundary.
    Committed,
    /// Commit returned after zero or more mutations and rollback is required.
    CommitFailed,
    /// Rollback restored the preflight snapshot.
    RolledBack,
    /// Rollback failed; temporary staging must still be cleaned up.
    RollbackFailed,
    /// Private staging was successfully removed.
    Cleaned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InstalledTarget<I> {
    pub(crate) target: OwnedTarget,
    pub(crate) fingerprint: I,
}

/// Identity-checked filesystem effects; the journal consumes a record only after success.
pub(crate) trait RollbackStore<I, D, U> {
    type Error;
    fn remove_installed(&mut self, target: &InstalledTarget<I>) -> Result<(), Self::Error>;
    fn restore_backup(&mut self, target: OwnedTarget) -> Result<(), Self::Error>;
    fn unidentified_directory(&self, directory: &U) -> Self::Error;
    fn remove_directory(&mut self, directory: &D) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MutationJournal<I, D, U> {
    state: FilesystemStageState,
    backed_up: Vec<OwnedTarget>,
    installed: Vec<InstalledTarget<I>>,
    created_directories: Vec<D>,
    unidentified_created_directories: Vec<U>,
}

impl<I, D, U> Default for MutationJournal<I, D, U> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I, D, U> MutationJournal<I, D, U> {
    pub(crate) fn new() -> Self {
        Self {
            state: FilesystemStageState::Uncommitted,
            backed_up: Vec::new(),
            installed: Vec::new(),
            created_directories: Vec::new(),
            unidentified_created_directories: Vec::new(),
        }
    }
    pub(crate) fn state(&self) -> FilesystemStageState {
        self.state
    }
    pub(crate) fn commit_failed(&mut self) {
        self.state = FilesystemStageState::CommitFailed;
    }
    pub(crate) fn committed(&mut self) {
        self.state = FilesystemStageState::Committed;
    }
    pub(crate) fn rollback_failed(&mut self) {
        self.state = FilesystemStageState::RollbackFailed;
    }
    pub(crate) fn rolled_back(&mut self) {
        self.state = FilesystemStageState::RolledBack;
    }
    pub(crate) fn cleaned(&mut self) {
        self.state = FilesystemStageState::Cleaned;
    }
    pub(crate) fn backups(&self) -> &[OwnedTarget] {
        &self.backed_up
    }
    pub(crate) fn installed(&self) -> &[InstalledTarget<I>] {
        &self.installed
    }
    pub(crate) fn created_directories(&self) -> &[D] {
        &self.created_directories
    }
    pub(crate) fn backed_up(&mut self, target: OwnedTarget) {
        self.backed_up.push(target);
    }
    pub(crate) fn installed_target(&mut self, target: InstalledTarget<I>) {
        self.installed.push(target);
    }
    pub(crate) fn directory_progress(&mut self) -> (&mut Vec<D>, &mut Vec<U>) {
        (
            &mut self.created_directories,
            &mut self.unidentified_created_directories,
        )
    }

    pub(crate) fn restore<S: RollbackStore<I, D, U>>(
        &mut self,
        store: &mut S,
    ) -> Result<(), S::Error> {
        while let Some(installed) = self.installed.last() {
            store.remove_installed(installed)?;
            self.installed.pop();
        }
        while let Some(target) = self.backed_up.last().copied() {
            store.restore_backup(target)?;
            self.backed_up.pop();
        }
        if let Some(directory) = self.unidentified_created_directories.first() {
            return Err(store.unidentified_directory(directory));
        }
        while let Some(directory) = self.created_directories.last() {
            store.remove_directory(directory)?;
            self.created_directories.pop();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
