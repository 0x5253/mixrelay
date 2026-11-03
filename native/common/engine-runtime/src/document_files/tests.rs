use super::*;
use mixrelay_engine::document_files::stage_json_limited;

#[test]
fn unpublished_cancel_and_failed_serialization_preserve_the_accepted_document() {
    let directory = mixrelay_test_support::FixtureDir::new("document-port").unwrap();
    let path = directory.join("catalog.json");
    std::fs::write(&path, b"accepted\n").unwrap();
    let staged = stage_json_limited(&DiskDocuments, &path, &42, 3).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"accepted\n");
    drop(staged);
    assert!(stage_json_limited(&DiskDocuments, &path, &42, 2).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"accepted\n");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    stage_json_limited(&DiskDocuments, &path, &42, 3)
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"42\n");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
}

#[test]
fn an_open_observation_retains_its_version_while_a_new_document_is_published() {
    let directory = mixrelay_test_support::FixtureDir::new("document-read").unwrap();
    let path = directory.join("workspace.mix");
    std::fs::write(&path, b"first version").unwrap();
    let mut observed = DiskDocuments.open(&path).unwrap();
    DiskDocuments
        .stage(
            &path,
            Box::new(|writer| writer.write_all(b"second version")),
        )
        .unwrap()
        .commit()
        .unwrap();
    let mut bytes = Vec::new();
    observed.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"first version");
    assert_eq!(std::fs::read(path).unwrap(), b"second version");
}
