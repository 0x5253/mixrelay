use super::*;
use mixrelay_audio_io::{process::ProcessingState, session::AudioBackendKind};

const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: 128,
};

fn runtime(authority: ClockAuthority, requires_rate: bool) -> AdapterRuntimeStatus {
    let mut profile = tests::profile(AudioBackendKind::CoreAudio);
    profile.clock_authority = authority;
    profile.requires_observed_device_rate = requires_rate;
    AdapterRuntimeStatus::new(profile, std::sync::Arc::new(mixrelay_time::StdClock::new()))
}

fn session(status: &AdapterRuntimeStatus) -> AudioSessionStatus {
    status.status(FORMAT).audio_session.unwrap()
}

#[test]
fn default_profiles_keep_existing_behavior_until_native_clock_evidence_arrives() {
    for authority in [
        ClockAuthority::EngineTimer,
        ClockAuthority::Device,
        ClockAuthority::AudioServer,
        ClockAuthority::Host,
    ] {
        let status = runtime(authority, false);
        let result = session(&status);
        assert_eq!(result.clock_authority, authority);
        assert_eq!(
            result.issue,
            (authority == ClockAuthority::EngineTimer)
                .then_some(SessionIssue::ClockNotSynchronized)
        );
        assert_eq!(result.processing_state, ProcessingState::Idle);
    }
}

#[test]
fn actual_device_pacing_is_not_itself_evidence_of_cross_domain_synchronization() {
    let status = runtime(ClockAuthority::EngineTimer, true);
    status.observe_device_rate(Some(48_000));
    let _lease = status.begin_processing().unwrap();
    for authority in [
        ClockAuthority::Device,
        ClockAuthority::AudioServer,
        ClockAuthority::Host,
    ] {
        status.observe_clock(authority, false);
        assert_eq!(session(&status).clock_authority, authority);
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::ClockNotSynchronized)
        );
        status.observe_clock(authority, true);
        assert_eq!(session(&status).issue, None);
        assert_eq!(session(&status).processing_state, ProcessingState::Running);
    }
    status.observe_clock(ClockAuthority::EngineTimer, false);
    assert_eq!(
        session(&status).issue,
        Some(SessionIssue::ClockNotSynchronized)
    );
}

#[test]
fn explicit_idle_is_neither_clock_failure_nor_synchronization_and_resumes_on_evidence() {
    let status = runtime(ClockAuthority::EngineTimer, false);
    let lease = status.begin_processing().unwrap();
    status.observe_clock(ClockAuthority::Device, false);
    assert_eq!(
        session(&status).issue,
        Some(SessionIssue::ClockNotSynchronized)
    );
    status.observe_clock_idle();
    let idle = session(&status);
    assert_eq!(idle.processing_state, ProcessingState::Idle);
    assert_eq!(
        idle.clock_authority,
        ClockAuthority::Device,
        "last observed authority is retained, not invented"
    );
    assert_eq!(idle.issue, None);
    assert!(
        !ClockObservation::unpack(status.clock_observation.load(Ordering::Acquire)).synchronized
    );
    status.observe_clock(ClockAuthority::Device, false);
    assert_eq!(session(&status).processing_state, ProcessingState::Running);
    assert_eq!(
        session(&status).issue,
        Some(SessionIssue::ClockNotSynchronized)
    );
    status.observe_clock(ClockAuthority::Device, true);
    assert_eq!(session(&status).issue, None);
    status.observe_clock_idle();
    assert!(
        !ClockObservation::unpack(status.clock_observation.load(Ordering::Acquire)).synchronized
    );
    status.request_stop();
    drop(lease);
    assert_eq!(session(&status).processing_state, ProcessingState::Stopped);
}

#[test]
fn idle_or_synchronized_clock_does_not_hide_worker_format_or_availability_errors() {
    for idle in [false, true] {
        let status = runtime(ClockAuthority::EngineTimer, true);
        let lease = status.begin_processing().unwrap();
        status.observe_clock(ClockAuthority::Device, true);
        if idle {
            status.observe_clock_idle();
        }
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::DeviceUnavailable)
        );
        status.observe_device_rate(Some(44_100));
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::SampleRateMismatch)
        );
        status.preferences_error.store(true, Ordering::Release);
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::PreferencesUnavailable)
        );
        status.observe_application_state(ApplicationRuntimeState::Waiting);
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::ApplicationUnavailable)
        );
        status.observe_application_state(ApplicationRuntimeState::Ambiguous);
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::ApplicationAmbiguous)
        );
        status.failed.store(true, Ordering::Release);
        assert_eq!(
            session(&status).issue,
            Some(SessionIssue::ReconfigurationFailed)
        );
        drop(lease);
        assert_eq!(session(&status).issue, Some(SessionIssue::ProcessingFailed));
        assert_eq!(session(&status).processing_state, ProcessingState::Stopped);
    }
}

#[test]
fn readers_cannot_mix_authority_and_synchronization_from_different_publications() {
    let status = Arc::new(runtime(ClockAuthority::EngineTimer, false));
    let running = status.begin_processing().unwrap();
    let writer_status = status.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..50_000 {
            writer_status.observe_clock(ClockAuthority::Device, true);
            writer_status.observe_clock_idle();
            writer_status.observe_clock(ClockAuthority::EngineTimer, false);
        }
    });
    for _ in 0..50_000 {
        let observed = session(&status);
        match (observed.clock_authority, observed.processing_state) {
            (ClockAuthority::Device, ProcessingState::Running | ProcessingState::Idle) => {
                assert_eq!(observed.issue, None);
            }
            (ClockAuthority::EngineTimer, ProcessingState::Running) => {
                assert_eq!(observed.issue, Some(SessionIssue::ClockNotSynchronized));
            }
            pair => panic!("mixed clock observation: {pair:?}"),
        }
    }
    writer.join().unwrap();
    status.request_stop();
    drop(running);
}
