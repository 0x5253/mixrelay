use super::*;
use std::time::{Duration, Instant};

fn selected_native() -> Fake<'static> {
    let native = Fake::default();
    native
        .selected
        .replace(vec![native::ProcessIdentity { object: 7, pid: 71 }]);
    native
}

fn at(session: &TapSession, native: &Fake<'_>, origin: Instant, seconds: u64) {
    assert_eq!(
        session.refresh_at(native, origin + Duration::from_secs(seconds)),
        Ok(())
    );
}

#[test]
fn diagnostic_audio_rebuilds_a_tap_that_started_without_any_callback() {
    let session = fallback_fixture();
    let native = selected_native();
    let ring = session.ring();
    let origin = Instant::now();
    at(&session, &native, origin, 0);
    at(&session, &native, origin, 2);
    assert_eq!(native.diagnostic_starts.get(), 0);
    at(&session, &native, origin, 3);
    assert!(native.diagnostic_live.get());
    assert_eq!(native.rebuild_calls.get(), 0);
    native.diagnostic_audio.set(true);
    at(&session, &native, origin, 4);
    assert!(!native.diagnostic_live.get());
    assert_eq!(native.diagnostic_retires.get(), 1);
    assert_eq!(native.rebuild_calls.get(), 1);
    assert_eq!(native.stop_calls.get(), 1);
    assert_eq!(session.ring(), ring);
    assert_eq!(session.stats().format_generation, 2);
    assert!(session.allowed() && session.stats().running);
    callback(&session, &mut [0.25, -0.25], None);
    let mut output = [0.0; 2];
    // SAFETY: the fixture owns the sole reader and one complete stereo frame.
    assert_eq!(
        unsafe { mr_process_tap_read(ptr::from_ref(&session).cast_mut(), output.as_mut_ptr(), 1) },
        1
    );
    assert_eq!(
        output.map(f32::to_bits),
        [0.25_f32, -0.25].map(f32::to_bits)
    );
    assert!(session.reclaim(&native));
    assert_eq!(native.diagnostic_retires.get(), 1);
}

#[test]
fn real_silence_never_restarts_capture_and_diagnostics_have_bounded_lifetimes() {
    let session = fallback_fixture();
    let native = selected_native();
    let origin = Instant::now();
    at(&session, &native, origin, 0);
    for _ in 0..8 {
        callback(&session, &mut [0.0; 2], None);
    }
    at(&session, &native, origin, 3);
    assert!(native.diagnostic_live.get());
    at(&session, &native, origin, 4);
    assert!(!native.diagnostic_live.get());
    for second in 5..14 {
        at(&session, &native, origin, second);
    }
    assert_eq!(native.diagnostic_starts.get(), 1);
    at(&session, &native, origin, 14);
    assert!(native.diagnostic_live.get());
    assert_eq!(native.diagnostic_starts.get(), 2);
    assert_eq!(native.rebuild_calls.get(), 0);
    assert_eq!(native.stop_calls.get(), 0);
    assert!(session.reclaim(&native));
    assert!(!native.diagnostic_live.get());
    assert_eq!(native.diagnostic_retires.get(), 2);
}

#[test]
fn resumed_audio_cancels_even_a_positive_diagnostic_before_or_during_observation() {
    for racing in [false, true] {
        let session = fallback_fixture();
        let native = Fake::default();
        native
            .selected
            .replace(vec![native::ProcessIdentity { object: 7, pid: 71 }]);
        let origin = Instant::now();
        at(&session, &native, origin, 0);
        at(&session, &native, origin, 3);
        native.diagnostic_audio.set(true);
        if racing {
            native.resume_during_diagnostic.set(Some(&session));
        } else {
            callback(&session, &mut [0.25, -0.25], None);
        }
        at(&session, &native, origin, 4);
        assert!(!native.diagnostic_live.get());
        assert_eq!(native.diagnostic_retires.get(), 1);
        assert_eq!(native.rebuild_calls.get(), 0);
        assert!(session.allowed() && session.stats().running);
        assert!(session.reclaim(&native));
    }
}

#[test]
fn diagnostics_preserve_the_exact_muted_lease_but_never_suppress_playback() {
    let session = muted_fixture();
    let native = selected_native();
    let origin = Instant::now();
    at(&session, &native, origin, 0);
    at(&session, &native, origin, 3);
    let probe = native.diagnostic_selection.borrow();
    let probe = probe.as_ref().expect("diagnostic selection");
    let original = session.selection.borrow();
    let original = original.as_ref().expect("original selection");
    assert!(!probe.mute);
    assert!(original.mute);
    assert_eq!(probe.bundle, original.bundle);
    assert_eq!(probe.selected, original.selected);
    assert_eq!(probe.target_device_uid, original.target_device_uid);
    assert_eq!(probe.target_device, original.target_device);
    // A changed lease revokes the original; watchdog evidence must not admit a
    // new process or move suppression onto a replacement output stream.
    native.device.set(native::DeviceLease {
        object: 7,
        stream: 71,
    });
    native.diagnostic_audio.set(true);
    assert_eq!(
        session.refresh_at(&native, origin + Duration::from_secs(4)),
        Err(TapError::code(9))
    );
    assert!(!session.allowed());
    assert!(!native.diagnostic_live.get());
    assert_eq!(native.rebuild_calls.get(), 0);
    assert!(session.reclaim(&native));
}

#[test]
fn probe_failure_preserves_the_current_capture_and_recovery_failure_stays_gated() {
    let session = fallback_fixture();
    let native = selected_native();
    let origin = Instant::now();
    at(&session, &native, origin, 0);
    native.fail.set(Failure::Diagnostic);
    at(&session, &native, origin, 3);
    assert!(session.allowed() && session.stats().running);
    assert_eq!(native.stop_calls.get(), 0);
    at(&session, &native, origin, 12);
    assert_eq!(native.diagnostic_starts.get(), 1);
    native.fail.set(Failure::None);
    at(&session, &native, origin, 13);
    assert!(native.diagnostic_live.get());
    native.diagnostic_audio.set(true);
    native.fail.set(Failure::Rebuild);
    assert_eq!(
        session.refresh_at(&native, origin + Duration::from_secs(14)),
        Err(TapError {
            code: 9,
            os_status: -1
        })
    );
    assert!(!session.allowed() && !session.stats().running);
    assert!(!native.diagnostic_live.get());
    native.fail.set(Failure::None);
    at(&session, &native, origin, 15);
    assert!(session.allowed() && session.stats().running);
    assert_eq!(session.stats().format_generation, 2);
    assert!(session.reclaim(&native));
}

#[test]
fn stop_and_pending_format_changes_retire_the_diagnostic_exactly_once() {
    for stop in [false, true] {
        let session = fallback_fixture();
        let native = selected_native();
        let origin = Instant::now();
        at(&session, &native, origin, 0);
        at(&session, &native, origin, 3);
        assert!(native.diagnostic_live.get());
        if stop {
            assert_eq!(session.stop(&native), Ok(()));
        } else {
            native.format.set(TapFormat {
                sample_rate: 44_100.0,
                ..format()
            });
            at(&session, &native, origin, 4);
        }
        assert!(!native.diagnostic_live.get());
        assert_eq!(native.diagnostic_retires.get(), 1);
        assert!(session.reclaim(&native));
        assert_eq!(native.diagnostic_retires.get(), 1);
    }
}

#[test]
fn live_pcm_evidence_survives_ring_overflow_and_independent_meter_drain() {
    let session = fallback_fixture();
    let native = selected_native();
    let origin = Instant::now();
    for second in 0..20 {
        callback(&session, &mut [0.25; 512], None);
        let mut peaks = [0.0; 2];
        session.peaks.take(&mut peaks);
        assert_eq!(peaks.map(f32::to_bits), [0.25_f32.to_bits(); 2]);
        at(&session, &native, origin, second);
    }
    assert_eq!(session.stats().captured_frames, 256);
    assert_eq!(session.stats().dropped_frames, 19 * 256);
    assert_eq!(native.diagnostic_starts.get(), 0);
    assert_eq!(native.rebuild_calls.get(), 0);
    assert!(session.reclaim(&native));
}

#[test]
fn bundle_selected_capture_recovers_without_requiring_legacy_process_objects() {
    let session = fallback_fixture();
    {
        let mut selection = session.selection.borrow_mut();
        let selection = selection.as_mut().expect("bundle selection");
        selection.fallback = false;
        selection.selected.clear();
    }
    let native = Fake::default();
    let origin = Instant::now();
    at(&session, &native, origin, 0);
    at(&session, &native, origin, 3);
    native.diagnostic_audio.set(true);
    at(&session, &native, origin, 4);
    assert_eq!(native.rebuild_calls.get(), 1);
    assert!(session.allowed() && session.stats().running);
    assert!(session.reclaim(&native));
}

#[test]
fn interrupted_recovery_keeps_full_rebuild_intent_across_every_native_boundary() {
    for fallback in [false, true] {
        for failure in [
            Failure::Unregister,
            Failure::Listener,
            Failure::Aggregate,
            Failure::Tap,
            Failure::Rebuild,
            Failure::Register,
            Failure::Start,
        ] {
            let session = fallback_fixture();
            if !fallback {
                let mut selection = session.selection.borrow_mut();
                let selection = selection.as_mut().expect("bundle selection");
                selection.fallback = false;
                selection.selected.clear();
            }
            let native = selected_native();
            let origin = Instant::now();
            at(&session, &native, origin, 0);
            at(&session, &native, origin, 3);
            native.diagnostic_audio.set(true);
            native.fail.set(failure);
            let error = session
                .refresh_at(&native, origin + Duration::from_secs(4))
                .unwrap_err();
            assert_eq!(error.code, 9, "{fallback:?}/{failure:?}");
            assert!(!session.allowed());
            assert_eq!(session.resources.get().rebuild, RebuildIntent::Replace);
            assert!(!native.diagnostic_live.get());
            native.fail.set(Failure::None);
            at(&session, &native, origin, 5);
            assert!(native.rebuild_calls.get() >= 1, "{fallback:?}/{failure:?}");
            assert_eq!(session.resources.get().rebuild, RebuildIntent::Unchanged);
            assert!(session.allowed() && session.stats().running);
            assert!(session.stats().format_generation >= 2);
            callback(&session, &mut [0.75, -0.75], None);
            let mut output = [0.0; 2];
            // SAFETY: the sole fixture reader owns one complete stereo frame.
            let frames = unsafe {
                mr_process_tap_read(ptr::from_ref(&session).cast_mut(), output.as_mut_ptr(), 1)
            };
            assert_eq!(frames, 1);
            assert_eq!(
                output.map(f32::to_bits),
                [0.75_f32, -0.75].map(f32::to_bits)
            );
            assert!(session.reclaim(&native));
            assert_eq!(native.diagnostic_retires.get(), 1);
        }
    }
}

#[test]
fn native_preparation_does_not_consume_the_diagnostic_observation_window() {
    let session = fallback_fixture();
    let native = selected_native();
    native.diagnostic_preparation.set(Duration::from_secs(2));
    let origin = Instant::now();
    at(&session, &native, origin, 0);
    at(&session, &native, origin, 3);
    // Native preparation finishes at 5 s; the complete observation window is
    // 5..6 s, regardless of the time at which control began preparing the tap.
    at(&session, &native, origin, 4);
    assert!(native.diagnostic_live.get());
    at(&session, &native, origin, 5);
    assert!(native.diagnostic_live.get());
    assert_eq!(native.diagnostic_retires.get(), 0);
    at(&session, &native, origin, 6);
    assert!(!native.diagnostic_live.get());
    assert_eq!(native.diagnostic_retires.get(), 1);
    assert_eq!(native.rebuild_calls.get(), 0);
    assert!(session.reclaim(&native));
}
