use super::*;
use mixrelay_processor_state::{StateBlobSource, StateStore};
use ports::ProcessorStates;

#[test]
fn a_retained_bundle_namespace_survives_selection_for_future_workers() {
    let directory = mixrelay_test_support::FixtureDir::new("state-port").unwrap();
    let files = HostStateFiles(ProcessorFiles::default());
    assert!(matches!(
        files.snapshot(),
        Err(ports::ProcessorStorageError::Unconfigured)
    ));
    let first = directory.join("first");
    files.select_namespace(&first);
    let snapshot = files.snapshot().unwrap();
    let reference = StateStore::new(first).put(&[1, 2, 3]).unwrap();
    files.select_namespace(&directory.join("next"));
    let mut captured = Vec::new();
    snapshot.copy_verified(&reference, &mut captured).unwrap();
    assert_eq!(captured, [1, 2, 3]);
    assert!(files.snapshot().unwrap().verify(&reference).is_err());
}
