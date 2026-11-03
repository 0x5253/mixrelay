use super::*;
use mixrelay_midi_control::Binding;
use std::fs;

fn binding() -> Binding {
    serde_json::from_value(serde_json::json!({"control":{"device":{"id":"disconnected","name":"Controller"},"channel":0,"kind":"cc","number":7},"target":{"graphId":"deleted-graph","nodeId":"deleted-node"},"action":"volume","mode":"absolute","stepDb":0.5,"graphName":"Programme","nodeName":"Microphone","minDb":-60,"maxDb":12})).unwrap()
}

#[test]
fn durable_round_trip_keeps_disconnected_and_deleted_targets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    let mut store = Store::load(repository(path.clone())).unwrap();
    assert!(store.bindings().is_empty());
    let mut second = binding();
    second.target.node_id = "another".into();
    second.mode = mixrelay_midi_control::Mode::Offset64;
    let entries = vec![binding(), second];
    store.save(entries.clone()).unwrap();
    assert_eq!(store.generation(), 1);
    let restored = Store::load(repository(path)).unwrap();
    assert_eq!(restored.bindings(), entries);
}

#[test]
fn invalid_versions_oversize_and_bad_options_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    for contents in [
        serde_json::json!({"version":2,"bindings":[]}),
        serde_json::json!({"version":1,"bindings":[{"control":null}]}),
    ] {
        fs::write(&path, contents.to_string()).unwrap();
        assert!(Store::load(repository(path.clone())).is_err());
    }
    fs::write(&path, vec![b' '; 1_048_577]).unwrap();
    assert!(Store::load(repository(path)).is_err());
}

#[test]
fn failed_save_does_not_publish_a_binding_or_generation() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("parent");
    let mut store = Store::load(repository(parent.join("midi.json"))).unwrap();
    fs::write(parent, b"not a directory").unwrap();
    assert!(store.save(vec![binding()]).is_err());
    assert!(store.bindings().is_empty());
    assert_eq!(store.generation(), 0);
}
