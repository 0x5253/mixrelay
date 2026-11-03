//! External adoption shares the accepted format, without writing the native rate back.
use super::*;
use mixrelay_audio_io::session::AudioBackendKind;

static FIXTURE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static RATE: AtomicU32 = AtomicU32::new(44_100);
static SETS: AtomicU32 = AtomicU32::new(0);

fn read_rate() -> Option<u32> {
    let value = RATE.load(Ordering::Acquire);
    (value != 0).then_some(value)
}

fn set_rate(value: u32) -> Result<(), String> {
    if value == 0 {
        return Err("invalid fixture rate".into());
    }
    SETS.fetch_add(1, Ordering::Relaxed);
    RATE.store(value, Ordering::Release);
    Ok(())
}

fn status() -> AdapterRuntimeStatus {
    let mut profile = tests::profile(AudioBackendKind::CoreAudio);
    profile.capabilities.can_reconfigure_live = true;
    AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
        .with_format_control(set_rate)
        .with_observed_rate_following(read_rate)
}

fn worker(status: &Arc<AdapterRuntimeStatus>, rate_on_pause: Option<u32>) -> AudioWorker {
    let cloned = status.clone();
    let thread = std::thread::spawn(move || {
        while !cloned.stop_requested() {
            if cloned.pause_requested() {
                if let Some(rate) = rate_on_pause {
                    RATE.store(rate, Ordering::Release);
                }
                let _ = cloned.checkpoint();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    AudioWorker::new(status.clone(), thread)
}

#[test]
fn external_adoption_saves_shared_format_without_calling_native_setter() {
    let _fixture = FIXTURE.lock().unwrap();
    RATE.store(44_100, Ordering::Release);
    SETS.store(0, Ordering::Release);
    let directory = mixrelay_test_support::FixtureDir::new("rate-preferences").unwrap();
    let path = directory.join("session.json");
    let mut status = status();
    status.test_set_preferences(Ok(std::sync::Arc::new(SessionPreferences::at(
        path.clone(),
        AudioBackendKind::CoreAudio,
    ))));
    let status = Arc::new(status);
    let _worker = worker(&status, None);
    let old = RenderFormat {
        sample_rate: 44_100,
        block_frames: 128,
    };
    let requested = RenderFormat {
        sample_rate: 48_000,
        ..old
    };
    status.reconfigure(old, requested).unwrap();
    status.activate_format(requested);
    status.resume();
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(SETS.load(Ordering::Acquire), 1);
    RATE.store(96_000, Ordering::Release);
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
    assert_eq!(SETS.load(Ordering::Acquire), 1);
    assert_ne!(std::fs::read(&path).unwrap(), saved);
    let restored = SessionPreferences::at(path.clone(), AudioBackendKind::CoreAudio)
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(restored.sample_rate, 96_000);
}

#[test]
fn external_rate_race_and_unsupported_format_do_not_publish_or_write_back() {
    let _fixture = FIXTURE.lock().unwrap();
    RATE.store(48_000, Ordering::Release);
    SETS.store(0, Ordering::Release);
    let status = Arc::new(status());
    let _worker = worker(&status, Some(96_000));
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
    assert_eq!(SETS.load(Ordering::Acquire), 0);
    assert!(
        status
            .adopt_external_format(RenderFormat {
                sample_rate: 22_050,
                ..requested
            })
            .is_err()
    );
    RATE.store(0, Ordering::Release);
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
    let _fixture = FIXTURE.lock().unwrap();
    RATE.store(48_000, Ordering::Release);
    SETS.store(0, Ordering::Release);
    let mut status = status();
    // A failed store resolution is injected without changing the host environment.
    status.test_set_preferences(Err("fixture read-only preferences".into()));
    let status = Arc::new(status);
    let _worker = worker(&status, None);
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
    assert_eq!(SETS.load(Ordering::Acquire), 0);
}

#[test]
fn proposed_format_capabilities_do_not_read_old_active_format() {
    let status = status();
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
    let _worker = worker(&status, None);
    let old = RenderFormat {
        sample_rate: 44_100,
        block_frames: 128,
    };
    let next = RenderFormat {
        sample_rate: 96_000,
        ..old
    };
    status.reconfigure(old, next).unwrap();
    status.activate_format(next);
    status.resume();
    let session = status.status(next).audio_session.unwrap();
    assert_eq!(session.format.sample_rate, 96_000);
    assert_eq!(session.observed_device_sample_rate, Some(48_000));
    assert!(session.capabilities.can_reconfigure_live);
}
