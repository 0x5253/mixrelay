use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::Arc,
    thread,
};

use mixrelay_test_support::FixtureDir;
use serde_json::json;

use super::*;

fn test_store() -> (FixtureDir, StateStore) {
    let fixture =
        FixtureDir::new("processor-state-store").expect("create checkout-local fixture directory");
    let store = StateStore::new(fixture.join("state"));
    (fixture, store)
}

#[test]
fn put_read_and_verify_use_content_addressed_private_files() {
    let (_temporary, store) = test_store();
    let payload = b"processor preset bytes";
    let reference = store.put(payload).expect("store payload");

    assert_eq!(store.read(&reference).expect("read payload"), payload);
    store.verify(&reference).expect("verify payload");
    assert_eq!(reference.byte_length(), payload.len() as u64);
    assert_eq!(reference.sha256().len(), 64);
    assert_eq!(
        fs::metadata(store.root()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let entry = store.root().join(format!("{}.state", reference.sha256()));
    let metadata = fs::metadata(entry).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(metadata.nlink(), 1);

    assert_eq!(
        store.put(payload).expect("reuse existing payload"),
        reference
    );
    let after_repeat =
        fs::metadata(store.root().join(format!("{}.state", reference.sha256()))).unwrap();
    assert_eq!(after_repeat.ino(), metadata.ino());
    assert_eq!(fs::read_dir(store.root()).unwrap().count(), 1);
}

#[test]
fn references_validate_on_creation_deserialization_and_each_store_access() {
    assert!(StateRef::new("A".repeat(64), 0).is_err());
    assert!(StateRef::new("0".repeat(64), MAX_STATE_BYTES as u64 + 1).is_err());
    assert!(
        serde_json::from_value::<StateRef>(json!({
            "sha256": "../outside",
            "byteLength": 1
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<StateRef>(json!({
            "sha256": "0".repeat(64),
            "byteLength": 1,
            "machinePath": "/outside"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<StateRef>(json!({
            "sha256": "0".repeat(64),
            "byteLength": MAX_STATE_BYTES as u64 + 1
        }))
        .is_err()
    );

    let (_temporary, _store) = test_store();
    let malformed: Result<StateRef, _> = serde_json::from_value(json!({
        "sha256": "0".repeat(64),
        "byteLength": MAX_STATE_BYTES as u64 + 1
    }));
    assert!(malformed.is_err());
}

#[test]
fn missing_and_corrupt_blobs_are_rejected() {
    let (_temporary, store) = test_store();
    let missing = StateRef::new("0".repeat(64), 1).unwrap();
    assert!(matches!(
        store.verify(&missing),
        Err(StateError::MissingBlob)
    ));

    let reference = store.put(b"original").unwrap();
    fs::write(
        store.root().join(format!("{}.state", reference.sha256())),
        b"modified",
    )
    .unwrap();
    assert!(matches!(
        store.verify(&reference),
        Err(StateError::Integrity)
    ));
    assert!(matches!(store.read(&reference), Err(StateError::Integrity)));
}

#[test]
fn reads_do_not_create_or_repair_a_missing_or_permissive_store_directory() {
    let (_temporary, store) = test_store();
    let reference = StateRef::new("0".repeat(64), 1).unwrap();

    assert!(matches!(
        store.verify(&reference),
        Err(StateError::MissingBlob)
    ));
    assert!(matches!(
        store.read(&reference),
        Err(StateError::MissingBlob)
    ));
    assert!(!store.root().exists());

    fs::create_dir(store.root()).unwrap();
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        store.verify(&reference),
        Err(StateError::UnsafeStoreDirectory)
    ));
    assert_eq!(
        fs::metadata(store.root()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn stable_blobs_reject_external_hard_links_and_metadata_changes() {
    let (fixture, store) = test_store();
    let reference = store.put(b"immutable").unwrap();
    let blob = store.root().join(format!("{}.state", reference.sha256()));
    let external_link = fixture.join("outside-link");
    fs::hard_link(&blob, &external_link).unwrap();
    assert!(matches!(
        store.verify(&reference),
        Err(StateError::UnsafeStoreEntry)
    ));
    fs::remove_file(external_link).unwrap();

    let file = File::open(&blob).unwrap();
    let before = file.metadata().unwrap();
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o400)).unwrap();
    let after = file.metadata().unwrap();
    assert!(!same_blob_metadata(&before, &after));
    assert!(matches!(
        store.verify(&reference),
        Err(StateError::UnsafeStoreEntry)
    ));
}

#[test]
fn symlinked_store_entry_is_rejected_without_opening_its_target() {
    let (temporary, store) = test_store();
    let reference = StateRef::new("0".repeat(64), 1).unwrap();
    fs::create_dir(store.root()).unwrap();
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let target = temporary.path().join("outside");
    fs::write(&target, b"x").unwrap();
    std::os::unix::fs::symlink(
        target,
        store.root().join(format!("{}.state", reference.sha256())),
    )
    .unwrap();
    assert!(matches!(store.read(&reference), Err(StateError::Io(_))));
}

#[test]
fn symlinked_store_path_components_are_rejected() {
    let (temporary, _store) = test_store();
    let alias = temporary.path().join("alias");
    std::os::unix::fs::symlink(temporary.path(), &alias).unwrap();
    let store = StateStore::new(alias.join("state"));

    assert!(matches!(store.put(b"x"), Err(StateError::Io(_))));
}

#[test]
fn state_reference_serializes_only_the_camel_case_contract() {
    let reference = StateRef::new("0".repeat(64), 12).unwrap();
    let value = serde_json::to_value(&reference).unwrap();
    assert_eq!(value["sha256"], reference.sha256());
    assert_eq!(value["byteLength"], 12);
    assert_eq!(
        serde_json::from_value::<StateRef>(value).unwrap(),
        reference
    );
}

#[test]
fn concurrent_puts_publish_one_verified_immutable_blob() {
    let (_temporary, store) = test_store();
    let store = Arc::new(store);
    let payload = Arc::new(vec![17_u8; 1024 * 1024]);
    let writers = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let payload = Arc::clone(&payload);
            thread::spawn(move || store.put(&payload).expect("concurrent put"))
        })
        .collect::<Vec<_>>();
    let references = writers
        .into_iter()
        .map(|writer| writer.join().expect("writer thread"))
        .collect::<Vec<_>>();
    assert!(
        references
            .iter()
            .all(|reference| reference == &references[0])
    );
    store
        .verify(&references[0])
        .expect("published blob verifies");
    assert_eq!(
        store.read(&references[0]).unwrap().as_slice(),
        payload.as_slice()
    );
}

#[test]
fn maximum_blob_round_trips_without_a_second_full_size_buffer() {
    let (_temporary, store) = test_store();
    let payload = (0..MAX_STATE_BYTES)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect::<Vec<_>>();
    let reference = store.put(&payload).expect("store maximum-size blob");
    drop(payload);

    let restored = store.read(&reference).expect("read maximum-size blob");
    assert_eq!(restored.len(), MAX_STATE_BYTES);
    assert!(
        restored
            .iter()
            .enumerate()
            .all(|(index, byte)| usize::from(*byte) == index % 251)
    );
    drop(restored);
    store
        .verify(&reference)
        .expect("stream-verify maximum blob");
}

#[test]
fn failed_stream_write_removes_its_private_staging_file() {
    let (_temporary, store) = test_store();
    let reference = StateRef::new("0".repeat(64), 2).unwrap();
    let mut truncated = &b"x"[..];
    assert!(
        store
            .store_payload(&mut truncated, 2, Some(&reference))
            .is_err()
    );
    assert_eq!(fs::read_dir(store.root()).unwrap().count(), 0);
}

#[test]
fn a_full_length_payload_with_the_wrong_digest_is_not_published() {
    let (_temporary, store) = test_store();
    let reference = StateRef::new("0".repeat(64), 1).unwrap();
    let mut payload = &b"x"[..];
    assert!(matches!(
        crate::StateBlobSink::store_verified(&store, &mut payload, &reference),
        Err(StateError::Integrity)
    ));
    assert!(store.verify(&reference).is_err());
    assert_eq!(fs::read_dir(store.root()).unwrap().count(), 0);
}

#[test]
fn temporary_files_are_created_with_exclusive_private_permissions() {
    let (_temporary, store) = test_store();
    let directory = store.open_directory(true).unwrap();
    let mut temporary = TemporaryBlob::create(&directory).unwrap();
    temporary.file.as_mut().unwrap().write_all(b"x").unwrap();
    let path = store.root().join(&temporary.name);
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
