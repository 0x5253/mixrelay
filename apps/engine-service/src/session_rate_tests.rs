//! External adoption shares the accepted format; the session never writes a device rate.
use super::*;
use mixrelay_audio_io::session::AudioBackendKind;

/// One test's own endpoint nominal rate; zero means the endpoint is unpublished.
fn endpoint(rate: u32) -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(rate))
}

fn status(endpoint: &Arc<AtomicU32>) -> AdapterRuntimeStatus {
    let mut profile = tests::profile(AudioBackendKind::CoreAudio);
    profile.capabilities.can_reconfigure_live = true;
    let endpoint = endpoint.clone();
    AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
        .with_graph_format_control()
        .with_observed_rate_following(move || {
            let rate = endpoint.load(Ordering::Acquire);
            (rate != 0).then_some(rate)
        })
}

fn worker(
    status: &Arc<AdapterRuntimeStatus>,
    endpoint: &Arc<AtomicU32>,
    rate_on_pause: Option<u32>,
) -> AudioWorker {
    let cloned = status.clone();
    let endpoint = endpoint.clone();
    let thread = std::thread::spawn(move || {
        while !cloned.stop_requested() {
            if cloned.pause_requested() {
                if let Some(rate) = rate_on_pause {
                    endpoint.store(rate, Ordering::Release);
                }
                let _ = cloned.checkpoint();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    AudioWorker::new(status.clone(), thread)
}

#[test]
fn external_adoption_saves_the_shared_format() {
    let rate = endpoint(44_100);
    let directory = mixrelay_test_support::FixtureDir::new("rate-preferences").unwrap();
    let path = directory.join("session.json");
    let mut status = status(&rate);
    status.test_set_preferences(Ok(std::sync::Arc::new(SessionPreferences::at(
        path.clone(),
        AudioBackendKind::CoreAudio,
    ))));
    let status = Arc::new(status);
    let _worker = worker(&status, &rate, None);
    let old = RenderFormat {
        sample_rate: 44_100,
        block_frames: 128,
    };
    let requested = RenderFormat {
        sample_rate: 48_000,
        ..old
    };
    status.reconfigure(requested).unwrap();
    status.activate_format(requested);
    status.resume();
    let saved = std::fs::read(&path).unwrap();
    rate.store(96_000, Ordering::Release);
    let external = RenderFormat {
        sample_rate: 96_000,
        ..old
    };
    status.adopt_external_format(external).unwrap();
    // Adoption is not publication: UI reports the old active DSP format until activation.
    assert_eq!(
        status
            .status(external)
            .audio_session
            .unwrap()
            .format
            .sample_rate,
        48_000
    );
    status.activate_format(external);
    status.resume();
    let session = status.status(external).audio_session.unwrap();
    assert_eq!(session.format.sample_rate, 96_000);
    assert_eq!(session.preferred_format.unwrap().sample_rate, 96_000);
    assert_ne!(std::fs::read(&path).unwrap(), saved);
    let restored = SessionPreferences::at(path.clone(), AudioBackendKind::CoreAudio)
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(restored.sample_rate, 96_000);
}

#[test]
fn external_rate_race_and_unsupported_format_do_not_publish() {
    let rate = endpoint(48_000);
    let status = Arc::new(status(&rate));
    let _worker = worker(&status, &rate, Some(96_000));
    let requested = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    assert!(status.adopt_external_format(requested).is_err());
    status.resume();
    assert_eq!(
        status.test_active_format(),
        RenderFormat {
            sample_rate: 44_100,
            ..requested
        }
    );
    assert!(
        status
            .adopt_external_format(RenderFormat {
                sample_rate: 22_050,
                ..requested
            })
            .is_err()
    );
    rate.store(0, Ordering::Release);
    assert_eq!(status.external_sample_rate(), None);
    assert!(status.adopt_external_format(requested).is_err());
}

#[test]
fn observed_status_alone_does_not_enable_external_following() {
    let status = AdapterRuntimeStatus::new(
        tests::profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    status.observe_device_rate(Some(48_000));
    assert_eq!(status.external_sample_rate(), None);
    assert!(
        status
            .adopt_external_format(RenderFormat {
                sample_rate: 48_000,
                block_frames: 128
            })
            .is_err()
    );
}

#[test]
fn externally_changed_rate_still_adopts_when_preferences_cannot_be_saved() {
    let rate = endpoint(48_000);
    let mut status = status(&rate);
    // A failed store resolution is injected without changing the host environment.
    status.test_set_preferences(Err("fixture read-only preferences".into()));
    let status = Arc::new(status);
    let _worker = worker(&status, &rate, None);
    let next = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    status.adopt_external_format(next).unwrap();
    status.activate_format(next);
    status.resume();
    let session = status.status(next).audio_session.unwrap();
    assert_eq!(session.issue, Some(SessionIssue::PreferencesUnavailable));
    assert_eq!(session.format.sample_rate, 48_000);
    assert_eq!(session.preferred_format.unwrap().sample_rate, 48_000);
}

#[test]
fn proposed_format_capabilities_do_not_read_old_active_format() {
    let status = status(&endpoint(44_100));
    let requested = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    assert!(
        status
            .status(requested)
            .audio_session
            .unwrap()
            .capabilities
            .application_capture
    );
    assert!(
        !status
            .capabilities_for_format(requested)
            .unwrap()
            .application_capture
    );
}

#[test]
fn graph_only_control_does_not_invent_a_native_rate_change() {
    let mut profile = tests::profile(AudioBackendKind::PipeWire);
    profile.capabilities.can_reconfigure_live = true;
    let status = Arc::new(
        AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
            .with_graph_format_control(),
    );
    status.observe_device_rate(Some(48_000));
    let _worker = worker(&status, &endpoint(0), None);
    let next = RenderFormat {
        sample_rate: 96_000,
        block_frames: 128,
    };
    status.reconfigure(next).unwrap();
    status.activate_format(next);
    status.resume();
    let session = status.status(next).audio_session.unwrap();
    assert_eq!(session.format.sample_rate, 96_000);
    assert_eq!(session.observed_device_sample_rate, Some(48_000));
    assert!(session.capabilities.can_reconfigure_live);
}

#[test]
fn a_failed_preference_save_rejects_the_change_and_keeps_the_stored_destination() {
    let rate = endpoint(48_000);
    let directory = mixrelay_test_support::FixtureDir::new("rate-commit-failure").unwrap();
    let path = directory.join("session.json");
    // A directory where the document belongs makes the save fail without touching it.
    std::fs::create_dir(&path).unwrap();
    let mut status = status(&rate);
    status.test_set_preferences(Ok(std::sync::Arc::new(SessionPreferences::at(
        path.clone(),
        AudioBackendKind::CoreAudio,
    ))));
    let status = Arc::new(status);
    let _worker = worker(&status, &rate, None);
    let next = RenderFormat {
        sample_rate: 48_000,
        block_frames: 256,
    };
    assert!(status.reconfigure(next).is_err());
    status.resume();
    assert!(path.is_dir());
    let session = status.status(next).audio_session.unwrap();
    assert_eq!(session.issue, Some(SessionIssue::PreferencesUnavailable));
    assert_eq!(session.preferred_format, None);
}
