use super::*;
use serde::{
    Serializer,
    ser::{Error, SerializeSeq},
};

// Catalog fixtures ignore the separate run marker and lock files.
fn stored_files(directory: &std::path::Path) -> usize {
    std::fs::read_dir(directory)
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name != "engine-run.json" && name != "engine-run.lock"
        })
        .count()
}

fn fixture() -> mixrelay_test_support::FixtureDir {
    mixrelay_test_support::FixtureDir::new("document-persistence").unwrap()
}

#[test]
fn state_document_staging_allocation_measurement() {
    let directory = fixture();
    let path = directory.join("states.json");
    let value = serde_json::json!({"nativeToken": "a".repeat(4 * 1024 * 1024)});
    let start = std::time::Instant::now();
    let allocation = allocation_counter::measure(|| {
        drop(stage_json_limited(&path, &value, 16 * 1024 * 1024).unwrap());
    });
    eprintln!(
        "state staging elapsed_us={} allocation={allocation:?}",
        start.elapsed().as_micros()
    );
    assert!(!path.exists());
    assert_eq!(stored_files(directory.path()), 0);
    assert!(allocation.bytes_max < 128 * 1024);
}

#[test]
fn complete_file_limit_includes_newline_and_preserves_previous_document() {
    let directory = fixture();
    let path = directory.join("bounded.json");
    // JSON string uses four bytes, then one newline: exactly five bytes on disk.
    stage_json_limited(&path, &"ab", 5)
        .unwrap()
        .commit()
        .unwrap();
    let accepted = fs::read(&path).unwrap();
    assert_eq!(accepted, b"\"ab\"\n");
    for limit in [0, 3, 4] {
        let error = stage_json_limited(&path, &"ab", limit).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), accepted);
        assert_eq!(stored_files(directory.path()), 1);
    }
}

#[test]
fn limited_writer_accounts_for_partial_writes_without_exceeding_budget() {
    let mut bytes = [0; 2];
    let mut writer = LimitedWriter {
        inner: bytes.as_mut_slice(),
        remaining: 3,
    };
    assert_eq!(writer.write(b"abc").unwrap(), 2);
    assert_eq!(writer.remaining, 1);
    assert_eq!(
        writer.write(b"de").unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(writer.remaining, 1);
}

#[test]
fn bounded_staging_serializes_only_once() {
    struct Counted(std::cell::Cell<u32>);
    impl Serialize for Counted {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.set(self.0.get() + 1);
            serializer.serialize_str("preset")
        }
    }
    let directory = fixture();
    let counted = Counted(std::cell::Cell::new(0));
    drop(stage_json_limited(&directory.join("one.json"), &counted, 64).unwrap());
    assert_eq!(counted.0.get(), 1);
}

#[test]
fn commit_replaces_only_after_explicit_acceptance_and_leaves_no_staged_file() {
    let directory = fixture();
    let path = directory.join("graphs.json");
    let accepted = b"{\"revision\":1}\n";
    fs::write(&path, accepted).unwrap();

    let staged = stage_json(&path, &serde_json::json!({"revision":2})).unwrap();
    assert_eq!(fs::read(&path).unwrap(), accepted);
    assert_eq!(staged.temporary.parent(), path.parent());
    staged.commit().unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"{\"revision\":2}\n");
    assert_eq!(stored_files(directory.path()), 1);
}

#[test]
fn cancelling_does_not_replace_or_create_the_destination() {
    let directory = fixture();
    let path = directory.join("audio-session.json");
    drop(stage_json(&path, &17).unwrap());
    assert!(!path.exists());
    fs::write(&path, b"accepted").unwrap();

    drop(stage_json(&path, &18).unwrap());

    assert_eq!(fs::read(&path).unwrap(), b"accepted");
    assert_eq!(stored_files(directory.path()), 1);
}

struct PartialSerializationFailure;

impl Serialize for PartialSerializationFailure {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(2))?;
        sequence.serialize_element(&"partial data")?;
        Err(S::Error::custom("fixture serialization failure"))
    }
}

#[test]
fn partial_serialization_failure_cleans_staging_without_touching_accepted_bytes() {
    let directory = fixture();
    let path = directory.join("graphs.json");
    fs::write(&path, b"accepted").unwrap();

    assert_eq!(
        stage_json(&path, &PartialSerializationFailure)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Other
    );

    assert_eq!(fs::read(&path).unwrap(), b"accepted");
    assert_eq!(stored_files(directory.path()), 1);
}

#[test]
fn failed_staging_preserves_new_parent_directory_but_removes_its_private_file() {
    let directory = fixture();
    let parent = directory.join("new-settings");
    let path = parent.join("graphs.json");
    assert!(stage_json(&path, &PartialSerializationFailure).is_err());
    // Directory creation is not part of file replacement. Another writer may have
    // adopted this directory; cancellation owns only its create_new staging file.
    assert!(parent.is_dir());
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    assert!(!path.exists());
}

#[test]
fn failed_commit_cleans_staging_and_preserves_the_destination() {
    let directory = fixture();
    let destination = directory.join("occupied-directory");
    fs::create_dir(&destination).unwrap();
    let marker = destination.join("keep");
    fs::write(&marker, b"accepted").unwrap();
    let staged = stage_json(&destination, &17).unwrap();
    let temporary = staged.temporary.clone();

    assert!(staged.commit().is_err());

    assert!(!temporary.exists());
    assert_eq!(fs::read(&marker).unwrap(), b"accepted");
    assert_eq!(stored_files(directory.path()), 1);
}

#[test]
fn cancelling_one_transaction_does_not_remove_another_transactions_file() {
    let directory = fixture();
    let path = directory.join("graphs.json");
    let first = stage_json(&path, &1).unwrap();
    let second = stage_json(&path, &2).unwrap();
    assert_ne!(first.temporary, second.temporary);

    drop(first);

    assert!(second.temporary.exists());
    second.commit().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"2\n");
    assert_eq!(stored_files(directory.path()), 1);
}

#[cfg(unix)]
#[test]
fn newly_created_configuration_storage_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = fixture();
    let path = directory.join("private/graphs.json");
    let staged = stage_json(&path, &1).unwrap();
    let parent = path.parent().unwrap();

    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&staged.temporary)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    staged.commit().unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
