use super::*;
use mixrelay_audio_io::recording::RecorderAction;
use mixrelay_audio_recorder::RecorderFailure;

#[test]
fn a_recording_port_rejection_preserves_the_create_new_source_and_existing_bytes() {
    let directory = mixrelay_test_support::FixtureDir::new("rec-port").unwrap();
    let path = directory.join("existing.wav");
    std::fs::write(&path, b"existing recording").unwrap();
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 1).unwrap();
    tap.capture(&[], 48_000);
    let backend = RegistryRecordingBackend(registry);
    let failure = backend
        .command(
            "rec",
            RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().to_owned()),
                sample_rate: None,
                encoding: None,
            },
        )
        .unwrap_err();
    let source = failure
        .original()
        .downcast_ref::<mixrelay_audio_recorder::RecorderError>()
        .unwrap();
    assert!(
        matches!(source.failure(), RecorderFailure::CreateFile { path: failed, source } if failed == &path && source.kind() == std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(path).unwrap(), b"existing recording");
}

#[test]
fn an_engine_hold_retains_the_native_take_until_the_paused_graph_releases_it() {
    let directory = mixrelay_test_support::FixtureDir::new("rec-hold").unwrap();
    let path = directory.join("held.wav");
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("rec", 1).unwrap();
    tap.capture(&[], 48_000);
    let backend = RegistryRecordingBackend(registry);
    backend
        .command(
            "rec",
            RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().to_owned()),
                sample_rate: None,
                encoding: Some(mixrelay_audio_io::recording::Encoding::Float32),
            },
        )
        .unwrap();
    tap.capture(&[0.25, 0.5], 48_000);
    let holds = backend.open_take_taps();
    drop(tap);
    backend.reap_unused().unwrap();
    assert_eq!(
        backend.status("rec").state,
        mixrelay_audio_io::recording::RecorderState::Recording
    );
    let paused = backend
        .command(
            "rec",
            RecorderRequest {
                action: RecorderAction::Pause,
                path: None,
                sample_rate: None,
                encoding: None,
            },
        )
        .unwrap();
    assert_eq!(
        (paused.state, paused.frames),
        (mixrelay_audio_io::recording::RecorderState::Paused, 2)
    );
    drop(holds);
    backend.reap_unused().unwrap();
    let mut reader = mixrelay_test_support::rf64::open(path).unwrap();
    assert_eq!(
        reader
            .samples::<f32>()
            .map(|sample| sample.unwrap().to_bits())
            .collect::<Vec<_>>(),
        [0.25_f32, 0.5].map(f32::to_bits)
    );
}
