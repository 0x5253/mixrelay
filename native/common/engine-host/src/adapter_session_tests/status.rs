//! Adapter observation, process reporting and worker lifetime regressions.

use super::*;
use mixrelay_audio_io::session::AudioBackendKind;

#[test]
fn audio_worker_finish_cancels_and_joins_before_releasing_the_owner() {
    let status = Arc::new(AdapterRuntimeStatus::new(
        profile(AudioBackendKind::CoreAudio),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    let weak = Arc::downgrade(&status);
    let observed = status.clone();
    let worker = std::thread::spawn(move || {
        while !observed.stop_requested() {
            std::thread::yield_now();
        }
    });
    let owner = AudioWorker::new(status, worker);

    assert_eq!(owner.finish(), Ok(()));
    assert!(weak.upgrade().is_none());
}

#[test]
fn audio_worker_finish_preserves_a_panicked_worker_reason() {
    let status = Arc::new(AdapterRuntimeStatus::new(
        profile(AudioBackendKind::CoreAudio),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    let weak = Arc::downgrade(&status);
    let worker = std::thread::Builder::new()
        .name("failed-audio-worker".into())
        .spawn(|| panic!("injected audio failure"))
        .unwrap();

    assert_eq!(
        AudioWorker::new(status, worker).finish(),
        Err("failed-audio-worker panicked: injected audio failure".into())
    );
    assert!(weak.upgrade().is_none());
}

#[test]
fn os_label_does_not_invent_device_health_or_supported_io() {
    for backend in [AudioBackendKind::CoreAudio, AudioBackendKind::PipeWire] {
        let status = AdapterRuntimeStatus::new(
            profile(backend),
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        );
        let format = RenderFormat {
            sample_rate: 44_100,
            block_frames: 128,
        };
        let initial = status.status(format);
        assert_eq!(initial.driver_state, DriverState::InstalledNotLoaded);
        let session = initial.audio_session.unwrap();
        assert_eq!(session.clock_authority, ClockAuthority::AudioServer);
        assert!(session.capabilities.hardware_input);
        assert!(!session.capabilities.hardware_output);
        assert!(session.capabilities.application_capture);
        status.observe_endpoint(true);
        assert_eq!(status.status(format).driver_state, DriverState::Ready);
        status.observe_device_rate(Some(44_100));
        status.forget_endpoint_observation();
        let detached = status.status(format);
        assert_eq!(detached.driver_state, DriverState::InstalledNotLoaded);
        assert_eq!(
            detached.audio_session.unwrap().observed_device_sample_rate,
            None
        );
        status.observe_endpoint(false);
        assert_eq!(
            status.status(format).driver_state,
            DriverState::NotInstalled
        );
        assert!(
            !status
                .status(RenderFormat {
                    sample_rate: 48_000,
                    ..format
                })
                .audio_session
                .unwrap()
                .capabilities
                .application_capture
        );
    }
}

#[test]
fn timer_capability_cannot_claim_synchronized_audio() {
    let mut descriptor = profile(AudioBackendKind::PipeWire);
    descriptor.clock_authority = ClockAuthority::EngineTimer;
    let status = AdapterRuntimeStatus::new(
        descriptor,
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    assert_eq!(
        status
            .status(RenderFormat {
                sample_rate: 48_000,
                block_frames: 128
            })
            .audio_session
            .unwrap()
            .issue,
        Some(SessionIssue::ClockNotSynchronized)
    );
}

#[test]
fn processing_owner_and_real_transfer_counters_have_a_lifetime() {
    let status = AdapterRuntimeStatus::new(
        profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    let lease = status.begin_processing().unwrap();
    assert!(status.begin_processing().is_err());
    status.record_process(mixrelay_audio_io::process::ProcessReport {
        input_starvations: 3,
        output_overflows: 2,
        discontinuities: 4,
        sample_faults: mixrelay_audio_io::process::SampleFaults {
            source_samples: 5,
            input_samples: 6,
            output_samples: 7,
            blocks: 2,
        },
    });
    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let running = status.status(format);
    assert_eq!((running.underruns, running.overruns), (3, 2));
    assert_eq!(running.audio_session.as_ref().unwrap().discontinuities, 4);
    assert_eq!(running.sample_faults.source_samples, 5);
    assert_eq!(running.sample_faults.input_samples, 6);
    assert_eq!(running.sample_faults.output_samples, 7);
    assert_eq!(running.sample_faults.blocks, 2);
    assert_eq!(
        running.audio_session.unwrap().processing_state,
        mixrelay_audio_io::process::ProcessingState::Running
    );
    drop(lease);
    assert_eq!(
        status.status(format).audio_session.unwrap().issue,
        Some(SessionIssue::ProcessingFailed)
    );
    assert!(status.begin_processing().is_err());
}

#[test]
fn sample_fault_totals_saturate_and_do_not_reset_on_clean_cycles() {
    let status = AdapterRuntimeStatus::new(
        profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    let faults = mixrelay_audio_io::process::SampleFaults {
        source_samples: u64::MAX - 1,
        input_samples: u64::MAX - 2,
        output_samples: u64::MAX - 3,
        blocks: u64::MAX - 4,
    };
    status.record_process(mixrelay_audio_io::process::ProcessReport {
        sample_faults: faults,
        ..Default::default()
    });
    status.record_process(mixrelay_audio_io::process::ProcessReport {
        sample_faults: mixrelay_audio_io::process::SampleFaults {
            source_samples: 8,
            input_samples: 8,
            output_samples: 8,
            blocks: 8,
        },
        ..Default::default()
    });
    status.record_process(mixrelay_audio_io::process::ProcessReport::default());
    assert_eq!(
        status
            .status(RenderFormat {
                sample_rate: 48_000,
                block_frames: 128
            })
            .sample_faults,
        mixrelay_audio_io::process::SampleFaults {
            source_samples: u64::MAX,
            input_samples: u64::MAX,
            output_samples: u64::MAX,
            blocks: u64::MAX,
        }
    );
}

#[test]
fn dropping_worker_cancels_and_joins_native_resource_ownership() {
    struct NativeResource(Arc<AtomicBool>);
    impl Drop for NativeResource {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let status = Arc::new(AdapterRuntimeStatus::new(
        profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    let released = Arc::new(AtomicBool::new(false));
    let thread_status = status.clone();
    let thread_released = released.clone();
    let (ready, receive) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let _resource = NativeResource(thread_released);
        let _lease = thread_status.begin_processing().unwrap();
        ready.send(()).unwrap();
        while !thread_status.stop_requested() {
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let owner = AudioWorker::new(status.clone(), thread);
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!released.load(Ordering::Acquire));
    drop(owner);
    assert!(released.load(Ordering::Acquire));
    assert_eq!(
        status
            .status(RenderFormat {
                sample_rate: 48_000,
                block_frames: 128
            })
            .audio_session
            .unwrap()
            .processing_state,
        mixrelay_audio_io::process::ProcessingState::Stopped
    );
    assert_eq!(
        status
            .status(RenderFormat {
                sample_rate: 48_000,
                block_frames: 128
            })
            .audio_session
            .unwrap()
            .issue,
        None
    );
}

#[test]
fn unwinding_is_processing_failure_even_if_cancellation_was_requested() {
    let status = AdapterRuntimeStatus::new(
        profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    status.observe_endpoint(true);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lease = status.begin_processing().unwrap();
        status.request_stop();
        panic!("native owner failed during shutdown");
    }));
    assert!(result.is_err());
    let report = status.status(RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    });
    // A failed DSP owner must not claim that the installed virtual driver vanished.
    assert_eq!(report.driver_state, DriverState::Ready);
    assert_eq!(
        report.audio_session.unwrap().issue,
        Some(SessionIssue::ProcessingFailed)
    );
}

#[test]
fn application_availability_recovers_without_hiding_processing_failure() {
    let status = AdapterRuntimeStatus::new(
        profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let lease = status.begin_processing().unwrap();
    for (state, issue) in [
        (
            ApplicationRuntimeState::Waiting,
            Some(SessionIssue::ApplicationUnavailable),
        ),
        (
            ApplicationRuntimeState::Ambiguous,
            Some(SessionIssue::ApplicationAmbiguous),
        ),
        (ApplicationRuntimeState::Ready, None),
    ] {
        status.observe_application_state(state);
        assert_eq!(status.status(format).audio_session.unwrap().issue, issue);
    }
    drop(lease);
    status.observe_application_state(ApplicationRuntimeState::Waiting);
    assert_eq!(
        status.status(format).audio_session.unwrap().issue,
        Some(SessionIssue::ProcessingFailed)
    );
}

#[test]
fn runtime_backend_quiesces_processing_and_retains_failed_rollback_pause() {
    let status = Arc::new(AdapterRuntimeStatus::new(
        profile(AudioBackendKind::PipeWire),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    let ready = Arc::new(std::sync::Barrier::new(2));
    let worker_status = Arc::clone(&status);
    let worker_ready = Arc::clone(&ready);
    let worker = AudioWorker::new(
        Arc::clone(&status),
        std::thread::spawn(move || {
            let _processing = worker_status.begin_processing().unwrap();
            worker_ready.wait();
            while !worker_status.stop_requested() {
                let _ = worker_status.checkpoint();
                std::thread::yield_now();
            }
        }),
    );
    ready.wait();
    let runtime: &dyn RuntimeBackend = status.as_ref();
    let suspended = runtime.suspend();
    let paused = status.pause_requested();
    runtime.fail_reconfiguration();
    runtime.resume();
    let retained_pause = status.pause_requested();
    let issue = runtime
        .status(RenderFormat {
            sample_rate: 44_100,
            block_frames: 128,
        })
        .audio_session
        .unwrap()
        .issue;
    // Reap the synthetic processing owner before any failed assertion unwinds.
    worker.finish().unwrap();
    assert!(
        suspended.is_ok(),
        "runtime suspension failed: {suspended:?}"
    );
    assert!(
        paused,
        "native changes require an acknowledged block boundary"
    );
    assert!(retained_pause, "failed rollback must remain suspended");
    assert_eq!(issue, Some(SessionIssue::ReconfigurationFailed));
}
