use super::*;

#[test]
fn a_refresh_between_stats_loads_cannot_certify_pcm_for_a_stale_rate() {
    let session = fixture();
    let timebase = mixrelay_realtime::Timebase::current();
    session.timebase.set(timebase).unwrap();
    let native = Fake::default();
    let mut pcm = [0.25; 64];
    let mut timestamp = native::Timestamp::default();
    timestamp.host = timebase.ticks(std::time::Duration::from_secs(1));
    timestamp.rate_scalar = 1.0;
    timestamp.flags = 7;
    let stale = session.stats_observed(|| {
        native.format.set(TapFormat {
            sample_rate: 44_100.0,
            ..format()
        });
        session.lifecycle.changed();
        session.refresh(&native).unwrap();
        callback(&session, &mut pcm, Some(&timestamp));
    });
    assert_eq!(stale.sample_rate.to_bits(), 48_000_f64.to_bits());
    let mut output = [0.0; 64];
    let mut times = [0; 32];
    let read = |stats: TapStats, output: &mut [f32], times: &mut [u64]| {
        // SAFETY: serialized fixture retains its session and two complete output spans.
        unsafe {
            ffi::mr_process_tap_read_timed(
                ptr::from_ref(&session).cast_mut(),
                output.as_mut_ptr(),
                times.as_mut_ptr(),
                32,
                TapReadGeneration {
                    format: stats.format_generation,
                    clock_breaks: stats.clock_discontinuities,
                },
            )
        }
    };
    assert_eq!(
        read(stale, &mut output, &mut times),
        0,
        "a new generation certified PCM with an old resampler rate"
    );
    assert!(
        stale.running && !stale.format_ready && !stale.clock_valid,
        "mixed stats published native clock credit"
    );
    let fresh = session.stats();
    assert!(fresh.running && fresh.format_ready);
    assert_eq!(fresh.sample_rate.to_bits(), 44_100_f64.to_bits());
    assert_eq!(read(fresh, &mut output, &mut times), 32);
    assert_eq!(output.map(f32::to_bits), pcm.map(f32::to_bits));
    assert!(times[31].abs_diff(1_000_000_000 + 31 * 1_000_000_000 / 44_100) <= 1);
}

#[test]
fn invalidation_suspends_admission_without_reporting_a_permanent_stop() {
    let session = fixture();
    assert!(session.stats().format_ready);
    session.lifecycle.changed();
    let pending = session.stats();
    assert!(pending.running);
    assert!(!pending.format_ready);
    let mut pcm = [0.25; 64];
    callback(&session, &mut pcm, None);
    assert_eq!(session.stats().captured_frames, 0);
    session.refresh(&Fake::default()).unwrap();
    let ready = session.stats();
    assert!(ready.running && ready.format_ready);
    callback(&session, &mut pcm, None);
    assert_eq!(session.stats().captured_frames, 32);
}
