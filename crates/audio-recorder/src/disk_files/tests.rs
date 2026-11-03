use super::DiskFiles;
use crate::recording_files::{FileFactory, RecordingStream, memory::MemoryFiles};
use std::{
    io::{self, Seek, SeekFrom, Write},
    path::Path,
};

fn write_extent(factory: &mut impl FileFactory, path: &Path) {
    let mut file = factory.create(path).unwrap();
    file.write_all(b"abc").unwrap();
    assert!(
        matches!(factory.create(path), Err(error) if error.kind() == io::ErrorKind::AlreadyExists)
    );
    file.seek(SeekFrom::Start(1)).unwrap();
    file.write_all(b"Q").unwrap();
    file.truncate(2).unwrap();
    assert_eq!(file.stream_position().unwrap(), 2);
    file.truncate(5).unwrap();
    assert_eq!(file.stream_position().unwrap(), 2);
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(b"Z").unwrap();
    file.flush().unwrap();
}

#[test]
fn memory_destinations_preserve_disk_create_new_and_extent_contracts() {
    let directory = mixrelay_test_support::FixtureDir::new("rec-files").unwrap();
    let path = directory.join("extent.wav");
    write_extent(&mut DiskFiles, &path);
    assert_eq!(std::fs::read(&path).unwrap(), b"aQ\0\0\0Z");
    let mut files = MemoryFiles::default();
    write_extent(&mut files, &path);
    assert_eq!(files.bytes(path), b"aQ\0\0\0Z");
    assert_eq!(files.len(), 1);
}
