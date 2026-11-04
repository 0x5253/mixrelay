use super::*;
use mixrelay_processor_state::{StateBlobSource, StateStore};
use ports::ProcessorStates;

#[test]
fn a_rescan_path_that_is_not_a_directory_is_refused_before_any_scan() {
    let directory = mixrelay_test_support::FixtureDir::new("rescan-path").unwrap();
    let file = directory.join("plugin.clap");
    std::fs::write(&file, b"").unwrap();
    let catalog = HostCatalog(ProcessorCatalog::default());
    let before = ports::ProcessorCatalog::protocol_catalog(&catalog);
    let error =
        ports::ProcessorCatalog::rescan(&catalog, &[directory.path().to_owned(), file.clone()])
            .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProcessorUnavailable);
    assert_eq!(error.parameters["path"], serde_json::json!(file));
    assert_eq!(ports::ProcessorCatalog::protocol_catalog(&catalog), before);
}

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
