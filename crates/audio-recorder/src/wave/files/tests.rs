use super::{FileFactory, WaveFile};
use crate::recording_files::faults::FaultFiles;
use crate::recording_files::memory::MemoryFiles;
use hound::{SampleFormat, WavSpec};
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

mod partial;

#[test]
fn write_flush_and_header_seek_faults_report_the_original_file_without_creating_parts() {
    let directory = Path::new("/recorder-rf64-faults");
    let files = MemoryFiles::default();
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    for fault in [1, 2, 3] {
        let path = directory.join(format!("fault-{fault}.wav"));
        let failure = Arc::new(AtomicU8::new(0));
        let mut file = WaveFile::new(
            &path,
            spec,
            &mut FaultFiles(files.clone(), Arc::clone(&failure)),
        )
        .unwrap();
        file.write_sample(0.25_f32).unwrap();
        failure.store(fault, Ordering::Relaxed);
        if fault == 1 {
            let error = file.write_sample(0.5_f32).unwrap_err();
            assert!(
                error.to_string().contains(path.to_str().unwrap())
                    && error.to_string().contains("disk full")
            );
            failure.store(0, Ordering::Relaxed);
            // A subsequent valid sample cannot bypass the sticky write failure.
            assert!(file.write_sample(0.75_f32).is_err());
        }
        let error = file.finalize().unwrap_err();
        assert!(error.to_string().contains(path.to_str().unwrap()));
        assert!(error.to_string().contains(match fault {
            1 => "disk full",
            2 => "flush failed",
            _ => "seek failed",
        }));
        if fault == 1 {
            let mut reader = files.reader(&path);
            assert_eq!(reader.duration(), 1);
            assert_eq!(
                reader.samples::<f32>().next().unwrap().unwrap().to_bits(),
                0.25_f32.to_bits()
            );
        }
    }
    assert_eq!(files.len(), 3);
}

#[test]
fn prefix_recovery_keeps_both_the_write_fault_and_the_extent_fault() {
    let directory = Path::new("/rec-recovery-fault");
    let files = MemoryFiles::default();
    let path = directory.join("fault.wav");
    let failure = Arc::new(AtomicU8::new(0));
    let mut file = WaveFile::new(
        &path,
        WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        },
        &mut FaultFiles(files.clone(), Arc::clone(&failure)),
    )
    .unwrap();
    file.write_sample(0.25_f32).unwrap();
    failure.store(1, Ordering::Relaxed);
    assert!(
        file.write_sample(0.5_f32)
            .unwrap_err()
            .to_string()
            .contains("disk full")
    );
    failure.store(4, Ordering::Relaxed);
    let error = file.finalize().unwrap_err();
    assert!(
        error.to_string().contains("disk full") && error.to_string().contains("truncate failed")
    );
    assert!(error.to_string().contains(path.to_str().unwrap()));
    let crate::RecorderFailure::Combined { primary, secondary } = error.failure() else {
        panic!("both structured failure sources must survive: {error:?}");
    };
    assert!(
        matches!(primary.failure(), crate::RecorderFailure::Write { path: failed, source: hound::Error::IoError(source) }
        if failed == &path && source.kind() == io::ErrorKind::Other)
    );
    assert!(
        matches!(secondary.failure(), crate::RecorderFailure::Finalize { path: failed, source }
        if failed == &path && source.kind() == io::ErrorKind::Other)
    );
}

#[test]
fn header_creation_failure_keeps_its_source_and_create_new_destination() {
    let files = MemoryFiles::default();
    let path = Path::new("/recorder-header/take.wav");
    let mut factory = FaultFiles(files.clone(), Arc::new(AtomicU8::new(1)));
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let Err(error) = WaveFile::new(path, spec, &mut factory) else {
        panic!("the initial header write must fail");
    };
    assert!(
        matches!(error.failure(), crate::RecorderFailure::CreateHeader { path: failed, source: hound::Error::IoError(source) }
        if failed == path && source.kind() == io::ErrorKind::Other)
    );
    assert!(files.bytes(path).is_empty());
    let Err(retry) = WaveFile::new(path, spec, &mut factory) else {
        panic!("an existing failed destination must not be overwritten");
    };
    assert!(
        matches!(retry.failure(), crate::RecorderFailure::CreateFile { path: failed, source }
        if failed == path && source.kind() == io::ErrorKind::AlreadyExists)
    );
    assert_eq!(files.len(), 1);
}
