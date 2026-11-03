//! Native command acknowledgement, capture races, autonomous retirement and final file ownership.

use super::{Encoding, RecorderAction, RecorderRegistry, RecorderRequest, RecorderState};
use mixrelay_test_support::FixtureDir;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn request(action: RecorderAction) -> RecorderRequest {
    RecorderRequest {
        action,
        path: None,
        sample_rate: None,
        encoding: None,
    }
}

fn start(registry: &RecorderRegistry, path: &Path, rate: u32, encoding: Encoding) {
    registry
        .command(
            "rec",
            RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().to_owned()),
                sample_rate: Some(rate),
                encoding: Some(encoding),
            },
        )
        .unwrap();
}

fn read(path: &Path) -> (hound::WavSpec, Vec<f32>) {
    let mut reader = mixrelay_test_support::rf64::open(path).unwrap();
    (
        reader.spec(),
        reader.samples::<f32>().map(Result::unwrap).collect(),
    )
}

#[test]
fn float_capture_pause_resume_and_stop_preserve_exact_interleaved_pcm() {
    let directory = FixtureDir::new("recorder-pause").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 2).unwrap();
    tap.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    let first = [0.25, -0.5, 0.125, -0.25];
    tap.capture(&first, 48_000);
    assert_eq!(
        registry
            .command("rec", request(RecorderAction::Pause))
            .unwrap()
            .frames,
        2
    );
    tap.capture(&[9.0; 8], 48_000);
    registry
        .command("rec", request(RecorderAction::Record))
        .unwrap();
    tap.capture(&first, 48_000);
    let stopped = registry
        .command("rec", request(RecorderAction::Stop))
        .unwrap();
    assert_eq!((stopped.state, stopped.frames), (RecorderState::Stopped, 4));
    let (spec, samples) = read(&path);
    assert_eq!((spec.channels, spec.sample_rate), (2, 48_000));
    assert_eq!(samples, first.repeat(2));
}

#[test]
fn stopped_rate_observation_and_create_new_never_overwrite_existing_data() {
    let directory = FixtureDir::new("recorder-create").unwrap();
    let path = directory.join("existing.wav");
    std::fs::write(&path, b"owned-existing-file").unwrap();
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 1).unwrap();
    let mut record = request(RecorderAction::Record);
    record.path = Some(path.to_str().unwrap().to_owned());
    assert!(registry.command("rec", record.clone()).is_err());
    tap.capture(&[], 48_000);
    assert!(registry.command("rec", record).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"owned-existing-file");
    assert!(
        registry
            .status("rec")
            .error
            .unwrap()
            .contains("cannot create")
    );
}

#[test]
fn same_identity_survives_preparation_discard_and_expired_tap_reprepare() {
    let directory = FixtureDir::new("recorder-reprepare").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    drop(registry.prepare("rec", 1).unwrap());
    let active = registry.prepare("rec", 1).unwrap();
    registry.reap_unused().unwrap();
    active.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    drop(registry.prepare("rec", 1).unwrap());
    registry.reap_unused().unwrap();
    assert_eq!(registry.status("rec").state, RecorderState::Recording);
    assert!(registry.prepare("rec", 2).is_err());
    active.capture(&[0.75; 8], 48_000);
    registry
        .command("rec", request(RecorderAction::Stop))
        .unwrap();
    assert_eq!(read(&path).1, [0.75; 8]);
}

#[test]
fn channel_replacement_preserves_the_retired_failed_takes_identity_path_and_error() {
    let directory = FixtureDir::new("recorder-replaced-error").unwrap();
    let path = directory.join("missing-parent/failed.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 1).unwrap();
    tap.capture(&[], 48_000);
    assert!(
        registry
            .command(
                "rec",
                RecorderRequest {
                    action: RecorderAction::Record,
                    path: Some(path.to_str().unwrap().into()),
                    sample_rate: None,
                    encoding: None
                }
            )
            .is_err()
    );
    drop(tap);
    let replacement = registry.prepare("rec", 2).unwrap();
    let error = registry.reap_unused().unwrap_err();
    assert!(
        error.to_string().contains("recorder rec")
            && error.to_string().contains(path.to_str().unwrap())
            && error.to_string().contains("cannot create")
    );
    let crate::RecorderFailure::Session {
        id,
        path: failed_path,
        source,
    } = error.failure()
    else {
        panic!("cleanup must retain its structured session context: {error:?}");
    };
    assert_eq!(id, "rec");
    assert_eq!(failed_path.as_deref(), path.to_str());
    assert!(
        matches!(source.failure(), crate::RecorderFailure::CreateFile { path: failed_path, source }
        if failed_path == &path && source.kind() == std::io::ErrorKind::NotFound)
    );
    replacement.capture(&[], 48_000);
    assert!(registry.prepare("rec", 1).is_err());
    assert!(registry.reap_unused().is_ok());
}

#[test]
fn stop_all_finalizes_live_taps_and_reports_failed_identity_and_path() {
    let directory = FixtureDir::new("recorder-shutdown").unwrap();
    let path = directory.join("good.wav");
    let bad = directory.join("missing-parent/bad.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 1).unwrap();
    tap.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    tap.capture(&[0.5; 4], 48_000);
    let failed = registry.prepare("failed-node", 1).unwrap();
    failed.capture(&[], 48_000);
    assert!(
        registry
            .command(
                "failed-node",
                RecorderRequest {
                    action: RecorderAction::Record,
                    path: Some(bad.to_str().unwrap().into()),
                    sample_rate: None,
                    encoding: None
                }
            )
            .is_err()
    );
    let error = registry.stop_all().unwrap_err();
    assert!(
        error.to_string().contains("failed-node")
            && error.to_string().contains(bad.to_str().unwrap())
    );
    assert_eq!(registry.status("rec").state, RecorderState::Stopped);
    tap.capture(&[1.0; 4], 48_000);
    assert_eq!(read(&path).1, [0.5; 4]);
}

#[test]
fn final_tap_retirement_finalizes_without_status_polling_or_explicit_stop() {
    let directory = FixtureDir::new("recorder-retire").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 1).unwrap();
    tap.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    tap.capture(&[0.5; 16], 48_000);
    drop(tap);
    let deadline = Instant::now() + Duration::from_secs(2);
    while registry.status("rec").state == RecorderState::Recording {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(read(&path).1, [0.5; 16]);
    registry.reap_unused().unwrap();
    assert_eq!(registry.status("rec").path, None);
}

#[test]
fn capture_does_not_allocate_or_deallocate_and_invalid_pcm_fails_only_recorder() {
    let directory = FixtureDir::new("recorder-realtime").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 2).unwrap();
    tap.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    let allocation = allocation_counter::measure(|| tap.capture(&[0.25; 4096], 48_000));
    assert_eq!(
        (
            allocation.count_total,
            allocation.count_current,
            allocation.bytes_total
        ),
        (0, 0, 0)
    );
    tap.capture(&[0.0; 3], 48_000);
    assert!(
        registry
            .command("rec", request(RecorderAction::Stop))
            .is_err()
    );
    assert_eq!(registry.status("rec").state, RecorderState::Failed);
}

#[test]
fn stop_racing_capture_finalizes_before_return_and_excludes_later_callbacks() {
    let directory = FixtureDir::new("recorder-race").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = Arc::new(registry.prepare("rec", 1).unwrap());
    tap.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    let other = Arc::clone(&tap);
    let producer = std::thread::spawn(move || {
        for _ in 0..64 {
            other.capture(&[0.125; 256], 48_000);
        }
    });
    let status = registry
        .command("rec", request(RecorderAction::Stop))
        .unwrap();
    producer.join().unwrap();
    tap.capture(&[1.0; 256], 48_000);
    assert_eq!(read(&path).1.len(), usize::try_from(status.frames).unwrap());
    assert!(
        read(&path)
            .1
            .iter()
            .all(|sample| sample.to_bits() == 0.125_f32.to_bits())
    );
}

#[test]
fn registry_preserves_all_frames_across_repeated_pause_resume_until_stop() {
    let directory = FixtureDir::new("registry-resume").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 2).unwrap();
    tap.capture(&[], 48_000);
    registry
        .command(
            "rec",
            RecorderRequest {
                path: Some(path.to_str().unwrap().into()),
                encoding: Some(Encoding::Float32),
                ..request(RecorderAction::Record)
            },
        )
        .unwrap();
    for quantum in 1..=188 {
        tap.capture(&[0.25; 512], 48_000);
        // Pause acknowledges worker progress without a clock or polling deadline.
        let paused = registry
            .command("rec", request(RecorderAction::Pause))
            .unwrap();
        assert_eq!(paused.frames, quantum * 256);
        if quantum < 188 {
            assert_eq!(paused.state, RecorderState::Paused);
            tap.capture(&[0.5; 512], 48_000);
            registry
                .command("rec", request(RecorderAction::Record))
                .unwrap();
        }
    }
    assert_eq!(registry.status("rec").state, RecorderState::Paused);
    let status = registry
        .command("rec", request(RecorderAction::Stop))
        .unwrap();
    assert_eq!(
        (status.state, status.frames, status.error),
        (RecorderState::Stopped, 48_128, None)
    );
    assert_eq!(
        mixrelay_test_support::rf64::inspect(&path).unwrap().frames,
        48_128
    );
}

#[test]
fn final_registry_clone_drains_and_joins_while_a_render_tap_remains_live() {
    let directory = FixtureDir::new("rec-owner").unwrap();
    let path = directory.join("take.wav");
    let registry = RecorderRegistry::threaded_files();
    let retained = registry.clone();
    let tap = registry.prepare("rec", 1).unwrap();
    tap.capture(&[], 48_000);
    start(&registry, &path, 48_000, Encoding::Float32);
    tap.capture(&[0.25; 2], 48_000);
    drop(registry);
    assert_eq!(retained.status("rec").state, RecorderState::Recording);
    tap.capture(&[0.5; 2], 48_000);
    drop(retained);
    assert_eq!(read(&path).1, [0.25, 0.25, 0.5, 0.5]);
    tap.capture(&[1.0; 4], 48_000);
    assert_eq!(read(&path).1, [0.25, 0.25, 0.5, 0.5]);
}
