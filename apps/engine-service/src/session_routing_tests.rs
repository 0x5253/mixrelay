//! Routing hardware and the own virtual endpoint are distinct format owners.
use super::*;
use mixrelay_audio_io::{
    formats::{BUFFER_SIZES, SAMPLE_RATES},
    session::AudioBackendKind,
};

fn runtime() -> AdapterRuntimeStatus {
    let mut profile = tests::profile(AudioBackendKind::CoreAudio);
    profile.capabilities.supported_sample_rates = SAMPLE_RATES.to_vec();
    profile.capabilities.supported_block_frames = BUFFER_SIZES.to_vec();
    profile.capabilities.can_reconfigure_live = true;
    profile.requires_observed_device_rate = true;
    AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
        .with_graph_format_control()
}

fn worker(status: &Arc<AdapterRuntimeStatus>) -> AudioWorker {
    let worker_status = status.clone();
    let worker = std::thread::spawn(move || {
        while !worker_status.stop_requested() {
            let _ = worker_status.checkpoint();
            std::thread::park_timeout(Duration::from_millis(1));
        }
    });
    AudioWorker::new(status.clone(), worker)
}

#[test]
fn hardware_adoption_does_not_depend_on_absent_or_legacy_hal_catalog() {
    for hal_rates in [&[][..], &[44_100, 48_000][..]] {
        let status = Arc::new(runtime());
        let _worker = worker(&status);
        status.observe_supported_sample_rates(hal_rates);
        status.observe_device_rate(Some(44_100));
        let requested = RenderFormat {
            sample_rate: 96_000,
            block_frames: 128,
        };
        status.adopt_routing_clock(requested).unwrap();
        status.activate_format(requested);
        status.resume();
        status.observe_clock(ClockAuthority::Device, true);
        let session = status.status(requested).audio_session.unwrap();
        assert_eq!(session.observed_device_sample_rate, Some(96_000));
        assert_eq!(session.capabilities.supported_sample_rates, [96_000]);
        assert_eq!(session.issue, None);
        status.observe_device_rate(Some(48_000));
        assert_eq!(
            status
                .status(requested)
                .audio_session
                .unwrap()
                .observed_device_sample_rate,
            Some(96_000)
        );
        status.observe_routing_clock(false, None);
        assert_eq!(
            status
                .status(requested)
                .audio_session
                .unwrap()
                .observed_device_sample_rate,
            Some(48_000)
        );
        assert!(
            status
                .adopt_routing_clock(RenderFormat {
                    sample_rate: 384_000,
                    ..requested
                })
                .is_err()
        );
    }
}

#[test]
fn hardware_buffer_persists_without_rewriting_virtual_rate_preference() {
    let directory = mixrelay_test_support::FixtureDir::new("routing-preferences").unwrap();
    let store = SessionPreferences::at(directory.join("session.json"), AudioBackendKind::CoreAudio);
    let original = RenderFormat {
        sample_rate: 44_100,
        block_frames: 128,
    };
    store
        .stage(SessionFormat {
            sample_rate: original.sample_rate,
            block_frames: original.block_frames,
        })
        .unwrap()
        .commit()
        .unwrap();
    let mut status = runtime();
    status.test_set_preferred_format(original);
    status.test_set_preferences(Ok(std::sync::Arc::new(store)));
    let status = Arc::new(status);
    let _worker = worker(&status);
    status.observe_supported_sample_rates(&[]);
    for frames in [64, 512, 2048] {
        let requested = RenderFormat {
            sample_rate: 96_000,
            block_frames: frames,
        };
        status.prepare_routing_buffer(requested).unwrap().commit();
        status.activate_format(requested);
        status.resume();
        let saved = status
            .test_preferences()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(
            saved,
            SessionFormat {
                sample_rate: 44_100,
                block_frames: frames
            }
        );
        assert_eq!(status.test_preferred_format(), Some(saved));
        assert_eq!(
            status
                .status(requested)
                .audio_session
                .unwrap()
                .format
                .sample_rate,
            96_000
        );
    }
}

#[test]
fn routing_buffer_persistence_error_is_not_reported_as_success() {
    let mut status = runtime();
    status.test_set_preferences(Err("fixture storage unavailable".into()));
    let requested = RenderFormat {
        sample_rate: 96_000,
        block_frames: 64,
    };
    assert!(status.prepare_routing_buffer(requested).is_err());
    assert!(!status.pause_requested());
    assert_eq!(status.test_preferred_format(), None);
    assert!(
        status
            .prepare_routing_buffer(RenderFormat {
                block_frames: 33,
                ..requested
            })
            .is_err()
    );
}

#[test]
fn saved_buffer_restores_for_hardware_even_when_own_hal_is_absent() {
    let status = Arc::new(runtime().with_observed_rate_following(|| None));
    status.observe_supported_sample_rates(&[]);
    let fallback = RenderFormat {
        sample_rate: 44_100,
        block_frames: 128,
    };
    let saved = RenderFormat {
        sample_rate: 48_000,
        block_frames: 64,
    };
    assert_eq!(
        status.test_restore_saved_format(saved, fallback),
        RenderFormat {
            block_frames: 64,
            ..fallback
        }
    );
    assert_eq!(
        status.status(fallback).audio_session.unwrap().issue,
        Some(SessionIssue::DeviceUnavailable)
    );
    let _worker = worker(&status);
    let hardware = RenderFormat {
        sample_rate: 96_000,
        block_frames: 64,
    };
    status.adopt_routing_clock(hardware).unwrap();
    status.activate_format(hardware);
    status.resume();
    status.observe_clock(ClockAuthority::Device, true);
    assert_eq!(status.status(hardware).audio_session.unwrap().issue, None);
    assert_eq!(
        status.test_preferred_format(),
        Some(SessionFormat {
            sample_rate: 48_000,
            block_frames: 64
        })
    );
}

#[test]
fn startup_follows_current_daw_rate_and_persists_it_without_a_device_write() {
    let directory = mixrelay_test_support::FixtureDir::new("startup-device-rate").unwrap();
    let store = SessionPreferences::at(directory.join("session.json"), AudioBackendKind::CoreAudio);
    let saved = RenderFormat {
        sample_rate: 48_000,
        block_frames: 32,
    };
    store
        .stage(SessionFormat {
            sample_rate: saved.sample_rate,
            block_frames: saved.block_frames,
        })
        .unwrap()
        .commit()
        .unwrap();
    let mut status = runtime().with_observed_rate_following(|| Some(96_000));
    status.test_set_preferences(Ok(std::sync::Arc::new(store)));
    // Enumeration may lag the device's own valid rate readback during startup.
    status.observe_supported_sample_rates(&[]);
    let actual = status.test_restore_saved_format(
        saved,
        RenderFormat {
            sample_rate: 44_100,
            block_frames: 128,
        },
    );
    assert_eq!(
        actual,
        RenderFormat {
            sample_rate: 96_000,
            block_frames: 32
        }
    );
    assert!(status.device_matches(96_000));
    assert!(!status.test_preferences_error());
    let stored = status
        .test_preferences()
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(stored.sample_rate, 96_000);
    assert_eq!(stored.block_frames, 32);
    assert_eq!(status.test_preferred_format(), Some(stored));
}

#[test]
fn startup_storage_failure_does_not_retune_or_mislabel_the_live_device() {
    let mut status = runtime().with_observed_rate_following(|| Some(96_000));
    status.test_set_preferences(Err("fixture read-only preferences".into()));
    let saved = RenderFormat {
        sample_rate: 48_000,
        block_frames: 256,
    };
    let actual = status.test_restore_saved_format(saved, saved);
    assert_eq!(actual.sample_rate, 96_000);
    assert_eq!(actual.block_frames, 256);
    assert!(status.test_preferences_error());
    assert_eq!(
        status.status(actual).audio_session.unwrap().issue,
        Some(SessionIssue::PreferencesUnavailable)
    );
}

#[test]
fn startup_rejects_invalid_saved_buffer_even_when_device_observation_is_valid() {
    let status = runtime().with_observed_rate_following(|| Some(96_000));
    let fallback = RenderFormat {
        sample_rate: 44_100,
        block_frames: 128,
    };
    assert_eq!(
        status.test_restore_saved_format(
            RenderFormat {
                sample_rate: 48_000,
                block_frames: 33,
            },
            fallback
        ),
        fallback
    );
    assert!(status.test_preferences_error());
}

#[test]
fn failed_outer_publication_keeps_callback_suspended_after_resume() {
    let status = Arc::new(runtime());
    let _worker = worker(&status);
    status.pause_at_boundary().unwrap();
    status.fail_reconfiguration();
    status.resume();
    assert!(status.pause_requested());
    assert!(status.checkpoint());
    assert_eq!(
        status
            .status(RenderFormat {
                sample_rate: 96_000,
                block_frames: 64
            })
            .audio_session
            .unwrap()
            .issue,
        Some(SessionIssue::ReconfigurationFailed)
    );
}
