//! A native format is a pair: rollback restores observed hardware state, not old DSP state.
use super::*;
use mixrelay_audio_io::session::AudioBackendKind;
use std::sync::Mutex;

fn format(rate: u32, frames: u32) -> RenderFormat {
    RenderFormat {
        sample_rate: rate,
        block_frames: frames,
    }
}

fn worker(status: &Arc<AdapterRuntimeStatus>, on_pause: impl Fn() + Send + 'static) -> AudioWorker {
    let cloned = status.clone();
    let thread = std::thread::spawn(move || {
        while !cloned.stop_requested() {
            if cloned.pause_requested() {
                on_pause();
                let _ = cloned.checkpoint();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    AudioWorker::new(status.clone(), thread)
}

fn profile() -> AdapterProfile {
    let mut value = tests::profile(AudioBackendKind::CoreAudio);
    value.capabilities.can_reconfigure_live = true;
    value.requires_observed_device_rate = true;
    value
}

#[test]
fn partial_native_pair_failure_rolls_back_actual_pair_not_old_dsp_size() {
    let old_native = format(44_100, 512);
    let old_dsp = format(44_100, 128);
    let requested = format(48_000, 256);
    let native = Arc::new(Mutex::new(old_native));
    let writes = Arc::new(Mutex::new(Vec::new()));
    let reader = native.clone();
    let setter = native.clone();
    let calls = writes.clone();
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_observed_format_following(move || Some(*reader.lock().unwrap()))
        .with_native_format_control(move |value| {
            calls.lock().unwrap().push(value);
            let mut actual = setter.lock().unwrap();
            actual.sample_rate = value.sample_rate;
            if value == requested {
                return Err("buffer write rejected after rate changed".into());
            }
            actual.block_frames = value.block_frames;
            Ok(())
        }),
    );
    let _worker = worker(&status, || {});
    assert!(status.reconfigure(old_dsp, requested).is_err());
    status.resume();
    assert_eq!(*native.lock().unwrap(), old_native);
    assert_eq!(*writes.lock().unwrap(), vec![requested, old_native]);
    assert_eq!(status.test_active_format(), old_dsp);
    assert!(!status.pause_requested());
}

#[test]
fn failed_pair_rollback_keeps_processing_suspended_and_reports_failure() {
    let native = Arc::new(Mutex::new(format(44_100, 512)));
    let reader = native.clone();
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_observed_format_following(move || Some(*reader.lock().unwrap()))
        .with_native_format_control(|_| Err("native endpoint vanished".into())),
    );
    let _worker = worker(&status, || {});
    assert!(
        status
            .reconfigure(format(44_100, 128), format(48_000, 256))
            .is_err()
    );
    status.resume();
    assert!(status.pause_requested());
    assert_eq!(
        status
            .status(format(44_100, 128))
            .audio_session
            .unwrap()
            .issue,
        Some(SessionIssue::ReconfigurationFailed)
    );
}

#[test]
fn external_buffer_only_change_is_observed_without_calling_pair_setter() {
    let native = Arc::new(Mutex::new(format(44_100, 256)));
    let reader = native.clone();
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_observed_format_following(move || Some(*reader.lock().unwrap()))
        .with_native_format_control(|_| panic!("external adoption must not invoke setter")),
    );
    let _worker = worker(&status, || {});
    let old = format(44_100, 128);
    let next = status.external_format(old).unwrap();
    assert_eq!(next, format(44_100, 256));
    let observed = status.status(old).audio_session.unwrap();
    assert_eq!(observed.observed_device_buffer_frames, Some(256));
    assert_eq!(observed.issue, Some(SessionIssue::BlockSizeMismatch));
    status.adopt_external_format(next).unwrap();
    status.activate_format(next);
    status.resume();
    assert_eq!(status.status(next).audio_session.unwrap().issue, None);
}

#[test]
fn late_external_buffer_change_rejects_old_prepared_pair() {
    let native = Arc::new(Mutex::new(format(44_100, 256)));
    let reader = native.clone();
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_observed_format_following(move || Some(*reader.lock().unwrap())),
    );
    let changed = native.clone();
    let _worker = worker(&status, move || changed.lock().unwrap().block_frames = 64);
    let target = status.external_format(format(44_100, 128)).unwrap();
    assert!(status.adopt_external_format(target).is_err());
    status.resume();
    assert_eq!(status.test_active_format(), format(44_100, 128));
    assert_eq!(status.external_format(target), Some(format(44_100, 64)));
}

#[test]
fn unknown_full_native_pair_rejects_before_any_native_write() {
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_observed_format_following(|| None)
        .with_native_format_control(|_| panic!("unknown actual pair cannot be safely restored")),
    );
    let _worker = worker(&status, || {});
    assert!(
        status
            .reconfigure(format(44_100, 128), format(48_000, 256))
            .is_err()
    );
    status.resume();
}

#[test]
fn preference_commit_failure_restores_observed_pair_and_preserves_destination() {
    let directory = mixrelay_test_support::FixtureDir::new("native-pair-save").unwrap();
    let occupied = directory.join("session.json");
    std::fs::create_dir_all(&occupied).unwrap();
    let previous = format(44_100, 512);
    let native = Arc::new(Mutex::new(previous));
    let reader = native.clone();
    let writer = native.clone();
    let mut status = AdapterRuntimeStatus::new(
        profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    )
    .with_observed_format_following(move || Some(*reader.lock().unwrap()))
    .with_native_format_control(move |value| {
        *writer.lock().unwrap() = value;
        Ok(())
    });
    status.test_set_preferences(Ok(std::sync::Arc::new(SessionPreferences::at(
        occupied.clone(),
        AudioBackendKind::CoreAudio,
    ))));
    let status = Arc::new(status);
    let _worker = worker(&status, || {});
    assert!(
        status
            .reconfigure(format(44_100, 128), format(48_000, 256))
            .is_err()
    );
    status.resume();
    assert_eq!(*native.lock().unwrap(), previous);
    assert!(occupied.is_dir());
    assert_eq!(
        status
            .status(format(44_100, 128))
            .audio_session
            .unwrap()
            .issue,
        Some(SessionIssue::PreferencesUnavailable)
    );
}

#[test]
fn setter_acknowledgement_without_matching_native_pair_is_not_success() {
    let actual = format(44_100, 512);
    let status = Arc::new(
        AdapterRuntimeStatus::new(
            profile(),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        )
        .with_observed_format_following(move || Some(actual))
        .with_native_format_control(|_| Ok(())),
    );
    let _worker = worker(&status, || {});
    assert!(
        status
            .reconfigure(format(44_100, 128), format(48_000, 256))
            .is_err()
    );
    status.resume();
    assert!(!status.test_failed());
    assert_eq!(status.external_format(actual), Some(actual));
}

#[test]
fn buffer_observation_is_reported_and_rate_only_adapters_clear_unknown_size() {
    let status = AdapterRuntimeStatus::new(
        profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    status.observe_device_format(Some(format(44_100, 512)));
    let observed = status.status(format(44_100, 128)).audio_session.unwrap();
    let wire = serde_json::to_value(&observed).unwrap();
    assert_eq!(wire["observedDeviceBufferFrames"], 512);
    status.observe_device_rate(Some(44_100));
    let rate_only = status.status(format(44_100, 128)).audio_session.unwrap();
    assert_eq!(rate_only.observed_device_buffer_frames, None);
    assert!(
        !serde_json::to_value(rate_only)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("observedDeviceBufferFrames")
    );
}
