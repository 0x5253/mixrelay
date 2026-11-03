use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Remove(OwnedTarget, u64),
    Restore(OwnedTarget),
    Directory(u64),
}
struct Store {
    effects: Vec<Effect>,
    fail_at: Option<usize>,
}
impl Store {
    fn apply(&mut self, effect: Effect) -> Result<(), &'static str> {
        if self.fail_at == Some(self.effects.len()) {
            return Err("mutation failed");
        }
        self.effects.push(effect);
        Ok(())
    }
}
impl RollbackStore<u64, u64, u64> for Store {
    type Error = &'static str;
    fn remove_installed(&mut self, target: &InstalledTarget<u64>) -> Result<(), Self::Error> {
        self.apply(Effect::Remove(target.target, target.fingerprint))
    }
    fn restore_backup(&mut self, target: OwnedTarget) -> Result<(), Self::Error> {
        self.apply(Effect::Restore(target))
    }
    fn unidentified_directory(&self, _: &u64) -> Self::Error {
        "unidentified directory"
    }
    fn remove_directory(&mut self, directory: &u64) -> Result<(), Self::Error> {
        self.apply(Effect::Directory(*directory))
    }
}
fn journal() -> MutationJournal<u64, u64, u64> {
    let mut journal = MutationJournal::new();
    journal.backed_up(OwnedTarget::App);
    journal.backed_up(OwnedTarget::Driver);
    journal.installed_target(InstalledTarget {
        target: OwnedTarget::App,
        fingerprint: 10,
    });
    journal.installed_target(InstalledTarget {
        target: OwnedTarget::Driver,
        fingerprint: 20,
    });
    journal.directory_progress().0.extend([1, 2]);
    journal
}

#[test]
fn rollback_removes_installed_files_before_restoring_backups_and_removing_directories() {
    let mut journal = journal();
    let mut store = Store {
        effects: Vec::new(),
        fail_at: None,
    };
    journal.restore(&mut store).unwrap();
    assert_eq!(
        store.effects,
        [
            Effect::Remove(OwnedTarget::Driver, 20),
            Effect::Remove(OwnedTarget::App, 10),
            Effect::Restore(OwnedTarget::Driver),
            Effect::Restore(OwnedTarget::App),
            Effect::Directory(2),
            Effect::Directory(1)
        ]
    );
    journal.restore(&mut store).unwrap();
    assert_eq!(store.effects.len(), 6);
}

#[test]
fn each_failed_effect_retains_its_receipt_and_resumes_without_repeating_successful_mutations() {
    for failure in 0..6 {
        let mut journal = journal();
        let mut store = Store {
            effects: Vec::new(),
            fail_at: Some(failure),
        };
        assert_eq!(journal.restore(&mut store), Err("mutation failed"));
        assert_eq!(store.effects.len(), failure);
        store.fail_at = None;
        journal.restore(&mut store).unwrap();
        assert_eq!(
            store.effects,
            [
                Effect::Remove(OwnedTarget::Driver, 20),
                Effect::Remove(OwnedTarget::App, 10),
                Effect::Restore(OwnedTarget::Driver),
                Effect::Restore(OwnedTarget::App),
                Effect::Directory(2),
                Effect::Directory(1)
            ]
        );
    }
}

#[test]
fn unidentified_directory_preserves_identified_directories_after_file_recovery() {
    let mut journal = journal();
    journal.directory_progress().1.push(3);
    let mut store = Store {
        effects: Vec::new(),
        fail_at: None,
    };
    assert_eq!(journal.restore(&mut store), Err("unidentified directory"));
    assert_eq!(store.effects.len(), 4);
    assert_eq!(journal.created_directories(), [1, 2]);
    assert!(journal.installed().is_empty());
    assert!(journal.backups().is_empty());
}
