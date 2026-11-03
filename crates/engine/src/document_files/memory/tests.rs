use super::*;
use crate::document_files::stage_json_limited;

#[test]
fn incomplete_or_rejected_writes_never_replace_the_accepted_document() {
    let files = MemoryDocuments::default();
    let path = Path::new("/catalog.json");
    files.insert(path, b"accepted".to_vec());
    drop(stage_json_limited(&files, path, &42, 3).unwrap());
    assert!(stage_json_limited(&files, path, &42, 2).is_err());
    assert_eq!(files.bytes(path).unwrap(), b"accepted");
    files.reject_commit(path, io::ErrorKind::PermissionDenied);
    let error = stage_json_limited(&files, path, &42, 3)
        .unwrap()
        .commit()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(files.bytes(path).unwrap(), b"accepted");
    files.allow_commit(path);
    stage_json_limited(&files, path, &42, 3)
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(files.bytes(path).unwrap(), b"42\n");
}

#[test]
fn open_observations_keep_their_original_version_across_commit_and_removal() {
    let files = MemoryDocuments::default();
    let path = Path::new("/catalog.json");
    files.insert(path, b"first".to_vec());
    let mut observed = files.open(path).unwrap();
    stage_json_limited(&files, path, &42, 3)
        .unwrap()
        .commit()
        .unwrap();
    files.remove(path).unwrap();
    let mut bytes = Vec::new();
    observed.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"first");
    assert!(matches!(files.open(path), Err(error) if error.kind() == io::ErrorKind::NotFound));
}
