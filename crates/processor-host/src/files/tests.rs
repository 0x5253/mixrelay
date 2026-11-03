use super::*;
use crate::worker_configuration::{read_worker_configuration, write_worker_config};
use mixrelay_test_support::FixtureDir;

#[test]
fn capture_destination_worker_startup_and_workspace_snapshot_share_the_selected_namespace() {
    let directory = FixtureDir::new("host-files").unwrap();
    let files = ProcessorFiles::default();
    assert!(matches!(
        files.prepare_write(),
        Err(StateStorageError::Unconfigured)
    ));
    files.select(StateStore::new(directory.join("states")));
    let registry = crate::ProcessorControlRegistry::new(Arc::new(files.clone()));
    let destination = registry.state_blobs.prepare_write().unwrap();
    let reference = destination.put(&[1, 2, 3]).unwrap();
    let parameters = serde_json::json!({"stateRef": reference});
    registry.verify_state(&parameters).unwrap();
    let snapshot = files.snapshot().unwrap();
    assert_eq!(snapshot.read(&reference).unwrap(), [1, 2, 3]);
    let path = directory.join("worker.json");
    write_worker_config(
        &path,
        48_000,
        &parameters,
        Some(&snapshot),
        &mixrelay_audio_graph::ProviderPorts::canonical_stereo("stereo"),
    )
    .unwrap();
    let prepared = read_worker_configuration(path.to_str().unwrap()).unwrap();
    assert_eq!(prepared.initial.native.unwrap(), [1, 2, 3]);
}

#[test]
fn prepared_capture_retains_its_destination_when_future_workers_select_another_store() {
    let directory = FixtureDir::new("host-files").unwrap();
    let files = ProcessorFiles::default();
    files.select(StateStore::new(directory.join("first")));
    let first = files.snapshot().unwrap();
    let destination = files.prepare_write().unwrap();
    files.select(StateStore::new(directory.join("second")));
    let reference = destination.put(&[4, 5, 6]).unwrap();
    assert_eq!(first.read(&reference).unwrap(), [4, 5, 6]);
    assert!(matches!(
        files.read(&reference),
        Err(StateStorageError::State(
            mixrelay_processor_state::StateError::MissingBlob
        ))
    ));
}
