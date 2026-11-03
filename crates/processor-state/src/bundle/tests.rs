use std::{cell::RefCell, collections::BTreeMap, io::Cursor};

use mixrelay_test_support::FixtureDir;

use super::*;
use crate::{StateStore, payload::hash_exact};

/// Payloads kept in memory and verified like the filesystem store verifies them.
#[derive(Default)]
struct MemoryBlobs(RefCell<BTreeMap<String, Vec<u8>>>);

impl MemoryBlobs {
    fn put(&self, bytes: &[u8]) -> StateRef {
        let digest = hash_exact(&mut &bytes[..], bytes.len() as u64, None, None, true).unwrap();
        let reference = StateRef::new(digest, bytes.len() as u64).unwrap();
        self.0
            .borrow_mut()
            .insert(reference.sha256().to_owned(), bytes.to_vec());
        reference
    }

    fn contains(&self, reference: &StateRef) -> bool {
        self.0.borrow().contains_key(reference.sha256())
    }
}

impl StateBlobSource for MemoryBlobs {
    fn verify(&self, reference: &StateRef) -> Result<(), StateError> {
        self.copy_verified(reference, &mut io::sink())
    }

    fn copy_verified(
        &self,
        reference: &StateRef,
        writer: &mut dyn Write,
    ) -> Result<(), StateError> {
        let blobs = self.0.borrow();
        let bytes = blobs
            .get(reference.sha256())
            .ok_or(StateError::MissingBlob)?;
        hash_exact(
            &mut &bytes[..],
            reference.byte_length(),
            Some(writer),
            Some(reference),
            true,
        )
        .map(|_| ())
    }
}

impl StateBlobSink for MemoryBlobs {
    fn store_verified(
        &self,
        reader: &mut dyn Read,
        reference: &StateRef,
    ) -> Result<(), StateError> {
        let mut bytes = Vec::new();
        hash_exact(
            reader,
            reference.byte_length(),
            Some(&mut bytes),
            Some(reference),
            false,
        )?;
        self.0
            .borrow_mut()
            .insert(reference.sha256().to_owned(), bytes);
        Ok(())
    }
}

fn sorted(mut references: Vec<StateRef>) -> Vec<StateRef> {
    references.sort_by(|left, right| left.sha256().cmp(right.sha256()));
    references
}

fn raw_bundle(manifest: &[u8], frames: &[(&StateRef, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    let manifest_length = u32::try_from(manifest.len()).expect("test manifest fits framing");
    let frame_count = u32::try_from(frames.len()).expect("test frame count fits framing");
    bytes.extend_from_slice(&manifest_length.to_le_bytes());
    bytes.extend_from_slice(&frame_count.to_le_bytes());
    bytes.extend_from_slice(manifest);
    for (reference, payload) in frames {
        bytes.extend_from_slice(&digest_bytes(reference).unwrap());
        bytes.extend_from_slice(&reference.byte_length().to_le_bytes());
        bytes.extend_from_slice(payload);
    }
    bytes
}

#[test]
fn a_round_trip_keeps_the_manifest_and_every_payload() {
    let source = MemoryBlobs::default();
    let target = MemoryBlobs::default();
    let entries = sorted(vec![source.put(b"one"), source.put(b"two")]);

    let mut archive = Vec::new();
    write_bundle(&mut archive, b"manifest", &entries, &source).unwrap();
    let (manifest, references) = read_bundle(Cursor::new(&archive), &target).unwrap();

    assert_eq!(manifest, b"manifest");
    assert_eq!(references, entries);
    assert!(
        references
            .iter()
            .all(|reference| target.contains(reference))
    );
}

#[test]
fn the_filesystem_store_writes_a_path_free_bundle_and_interns_it_elsewhere() {
    let fixture = FixtureDir::new("processor-state-bundle").expect("create fixture directory");
    let source = StateStore::new(fixture.join("source"));
    let target = StateStore::new(fixture.join("target"));
    let entries = sorted(vec![
        source.put(b"one").unwrap(),
        source.put(b"two").unwrap(),
    ]);

    let mut archive = Vec::new();
    write_bundle(&mut archive, b"manifest", &entries, &source).unwrap();
    let (manifest, references) = read_bundle(Cursor::new(&archive), &target).unwrap();

    assert_eq!(manifest, b"manifest");
    assert_eq!(references, entries);
    for reference in &entries {
        target.verify(reference).unwrap();
    }
    let path_bytes = source.root().as_os_str().as_encoded_bytes();
    assert!(
        !archive
            .windows(path_bytes.len())
            .any(|window| window == path_bytes)
    );
}

#[test]
fn preview_verifies_payloads_without_keeping_them() {
    let source = MemoryBlobs::default();
    let reference = source.put(b"portable");
    let mut archive = Vec::new();
    write_bundle(
        &mut archive,
        b"m",
        std::slice::from_ref(&reference),
        &source,
    )
    .unwrap();

    let (manifest, references) = preview_bundle(Cursor::new(archive)).unwrap();

    assert_eq!(manifest, b"m");
    assert_eq!(references, [reference]);
}

#[test]
fn writer_rejects_duplicate_unsorted_and_missing_references_before_writing() {
    let blobs = MemoryBlobs::default();
    let [lower, higher]: [StateRef; 2] = sorted(vec![blobs.put(b"first"), blobs.put(b"second")])
        .try_into()
        .unwrap();
    let mut output = Vec::new();
    assert!(write_bundle(&mut output, b"m", &[lower.clone(), lower.clone()], &blobs).is_err());
    assert!(write_bundle(&mut output, b"m", &[higher, lower], &blobs).is_err());
    let missing = MemoryBlobs::default().put(b"elsewhere");
    assert!(matches!(
        write_bundle(&mut output, b"m", &[missing], &blobs),
        Err(StateError::MissingBlob)
    ));
    assert!(output.is_empty());
}

#[test]
fn reader_rejects_duplicate_references_truncation_corruption_and_trailing_data() {
    let reference = StateRef::new(
        "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
        1,
    )
    .unwrap();
    let duplicate = raw_bundle(b"m", &[(&reference, b"x"), (&reference, b"x")]);
    assert!(preview_bundle(Cursor::new(duplicate)).is_err());

    let valid = raw_bundle(b"m", &[(&reference, b"x")]);
    assert!(preview_bundle(Cursor::new(&valid)).is_ok());
    assert!(preview_bundle(Cursor::new(&valid[..valid.len() - 1])).is_err());
    let mut corrupted = valid.clone();
    *corrupted.last_mut().unwrap() = b'y';
    assert!(preview_bundle(Cursor::new(corrupted)).is_err());
    let mut trailing = valid;
    trailing.push(0);
    assert!(preview_bundle(Cursor::new(trailing)).is_err());
}

#[test]
fn reader_rejects_unknown_magic_and_versions() {
    let mut magic = raw_bundle(b"m", &[]);
    magic[0] ^= 1;
    assert!(preview_bundle(Cursor::new(magic)).is_err());
    let mut version = raw_bundle(b"m", &[]);
    version[MAGIC.len()] ^= 1;
    assert!(preview_bundle(Cursor::new(version)).is_err());
}

#[test]
fn reader_rejects_oversized_state_references_before_payload() {
    let mut oversized = Vec::new();
    oversized.extend_from_slice(MAGIC);
    oversized.extend_from_slice(&VERSION.to_le_bytes());
    oversized.extend_from_slice(&0_u32.to_le_bytes());
    oversized.extend_from_slice(&1_u32.to_le_bytes());
    oversized.extend_from_slice(&[0_u8; 32]);
    oversized.extend_from_slice(&((crate::MAX_STATE_BYTES as u64) + 1).to_le_bytes());
    assert!(preview_bundle(Cursor::new(oversized)).is_err());
}

#[test]
fn bundle_limits_reject_oversized_manifest_count_and_combined_payload() {
    let mut manifest = Vec::new();
    manifest.extend_from_slice(MAGIC);
    manifest.extend_from_slice(&VERSION.to_le_bytes());
    let oversized_manifest =
        u32::try_from(MAX_BUNDLE_MANIFEST_BYTES + 1).expect("test bound fits framing");
    manifest.extend_from_slice(&oversized_manifest.to_le_bytes());
    manifest.extend_from_slice(&0_u32.to_le_bytes());
    assert!(preview_bundle(Cursor::new(manifest)).is_err());

    let mut count = Vec::new();
    count.extend_from_slice(MAGIC);
    count.extend_from_slice(&VERSION.to_le_bytes());
    count.extend_from_slice(&0_u32.to_le_bytes());
    let oversized_count =
        u32::try_from(MAX_BUNDLE_REFERENCES + 1).expect("test bound fits framing");
    count.extend_from_slice(&oversized_count.to_le_bytes());
    assert!(preview_bundle(Cursor::new(count)).is_err());

    let references = (1..=17)
        .map(|digest| {
            StateRef::new(format!("{digest:064x}"), crate::MAX_STATE_BYTES as u64).unwrap()
        })
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    assert!(write_bundle(&mut output, b"m", &references, &MemoryBlobs::default()).is_err());
    assert!(output.is_empty());
}

#[test]
fn a_bad_later_frame_keeps_only_the_payloads_verified_before_it() {
    let source = MemoryBlobs::default();
    let target = MemoryBlobs::default();
    let mut frames = [
        (source.put(b"valid"), b"valid".as_slice()),
        (source.put(b"invalid"), b"invalid".as_slice()),
    ];
    frames.sort_by(|left, right| left.0.sha256().cmp(right.0.sha256()));
    let mut archive = raw_bundle(
        b"m",
        &frames
            .iter()
            .map(|(reference, payload)| (reference, *payload))
            .collect::<Vec<_>>(),
    );
    let last = archive.len() - 1;
    archive[last] ^= 1;

    assert!(read_bundle(Cursor::new(archive), &target).is_err());
    assert!(target.contains(&frames[0].0));
    assert!(!target.contains(&frames[1].0));
}

/// Claims success without reading, as a careless deduplicating sink might.
struct SkippingSink;

impl StateBlobSink for SkippingSink {
    fn store_verified(&self, _: &mut dyn Read, _: &StateRef) -> Result<(), StateError> {
        Ok(())
    }
}

#[test]
fn a_sink_must_read_each_payload_to_its_length() {
    let source = MemoryBlobs::default();
    let reference = source.put(b"payload");
    let mut archive = Vec::new();
    write_bundle(
        &mut archive,
        b"m",
        std::slice::from_ref(&reference),
        &source,
    )
    .unwrap();
    assert!(matches!(
        read_bundle(Cursor::new(archive), &SkippingSink),
        Err(StateError::InvalidBundle { .. })
    ));
}
