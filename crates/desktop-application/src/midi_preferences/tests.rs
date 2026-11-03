use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
#[derive(Debug, Default)]
struct Memory {
    bytes: Mutex<Option<Vec<u8>>>,
    fail: AtomicBool,
}
impl PreferencesRepository for Memory {
    fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.bytes.lock().unwrap().clone())
    }
    fn replace(&self, bytes: &[u8]) -> std::io::Result<()> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("commit failed"));
        }
        *self.bytes.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
}
fn binding() -> Binding {
    serde_json::from_value(serde_json::json!({"control":{"device":{"id":"disconnected","name":"Controller"},"channel":0,"kind":"cc","number":7},"target":{"graphId":"deleted-graph","nodeId":"deleted-node"},"action":"volume","mode":"absolute","stepDb":0.5,"graphName":"Programme","nodeName":"Microphone","minDb":-60,"maxDb":12})).unwrap()
}
#[test]
fn failed_commit_preserves_saved_bindings_and_generation_then_retry_publishes_once() {
    let repository = Arc::new(Memory::default());
    let mut store = BindingStore::load(repository.clone()).unwrap();
    store.save(vec![binding()]).unwrap();
    let before = repository.read().unwrap();
    let mut changed = binding();
    changed.target.node_id = "another".into();
    repository.fail.store(true, Ordering::Relaxed);
    assert!(matches!(
        store.save(vec![changed.clone()]),
        Err(MidiPreferencesError::Io(_))
    ));
    assert_eq!(store.bindings(), [binding()]);
    assert_eq!(store.generation(), 1);
    assert_eq!(repository.read().unwrap(), before);
    repository.fail.store(false, Ordering::Relaxed);
    store.save(vec![changed.clone()]).unwrap();
    assert_eq!(store.generation(), 2);
    assert_eq!(
        BindingStore::load(repository).unwrap().bindings(),
        [changed]
    );
}
#[test]
fn oversized_valid_document_and_invalid_bindings_never_replace_previous_memory_or_bytes() {
    let repository = Arc::new(Memory::default());
    let mut store = BindingStore::load(repository.clone()).unwrap();
    store.save(vec![binding()]).unwrap();
    let before = repository.read().unwrap();
    let mut invalid = binding();
    invalid.control.channel = 16;
    assert!(matches!(
        store.save(vec![invalid]),
        Err(MidiPreferencesError::Binding(_))
    ));
    assert_eq!(repository.read().unwrap(), before);
    assert_eq!(store.bindings(), [binding()]);
    assert_eq!(store.generation(), 1);
    let mut bytes = serde_json::to_vec(&Saved {
        version: 1,
        bindings: vec![],
    })
    .unwrap();
    bytes.resize(FILE_LIMIT + 1, b' ');
    assert!(serde_json::from_slice::<Saved>(&bytes).is_ok());
    *repository.bytes.lock().unwrap() = Some(bytes.clone());
    assert!(matches!(
        BindingStore::load(repository.clone()),
        Err(MidiPreferencesError::Preferences)
    ));
    assert_eq!(repository.read().unwrap(), Some(bytes));
}
