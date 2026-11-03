use super::*;

fn native(frames: u32, rate: u32) -> DriverCycle {
    DriverCycle {
        frames,
        rate,
        position: 0,
        monotonic_ns: 0,
        clock_id: 17,
    }
}

fn loopback(rate: u32) -> Loopback {
    Loopback {
        clock: std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        render: None,
        capture: None,
        render_channels: 2,
        capture_channels: 1,
        input: vec![0.25; MAX_FRAMES * 2].into_boxed_slice(),
        output: vec![0.75; MAX_FRAMES].into_boxed_slice(),
        mapper: FrameBudgetMapper::new(rate).unwrap(),
        sample_rate: rate,
        failure: None,
    }
}

#[test]
fn raw_budget_uses_converted_duration_and_reserves_fractional_carry() {
    for cycle in [native(8192, 48_000), native(32_768, 192_000)] {
        assert_eq!(budget::validate(&cycle, 192_000), Ok(()));
    }
    let boundary = native(8193, 48_000);
    assert_eq!(
        budget::validate(&boundary, 192_000),
        Err(DriverProcessFailure::FrameBudget {
            native_frames: 8193,
            native_rate: 48_000,
            stream_rate: 192_000,
            capacity_frames: 32_768,
        })
    );
    assert!(budget::validate(&native(32_768, 8_000), 192_000).is_err());
    // A floor-only check admits 32768 here; fractional carry requires frame 32769.
    assert!(budget::validate(&native(32_768, 48_001), 48_002).is_err());
    assert!(budget::validate(&native(1, 0), 192_000).is_err());
    assert!(budget::validate(&native(128, 48_000), 0).is_err());
}

#[test]
fn extreme_rate_ratio_latches_only_its_source_and_never_touches_pcm_or_mapper() {
    let mut affected = loopback(192_000);
    let mut peer = loopback(48_000);
    let cycle = native(32_768, 8_000);
    let mut failed = None;
    let allocation = allocation_counter::measure(|| {
        affected.process(&cycle);
        failed = affected.failure();
        affected.process(&native(128, 48_000));
        peer.process(&native(128, 48_000));
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(failed, Some(budget::failure(&cycle, 192_000)));
    assert_eq!(affected.failure(), failed);
    assert!(
        affected
            .input
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
    assert!(
        affected
            .output
            .iter()
            .all(|sample| sample.to_bits() == 0.75_f32.to_bits())
    );
    let mapped = affected
        .mapper
        .map(ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::AudioServer,
                id: 17,
                epoch: 0,
            },
            sample_rate: 48_000,
            frames: 128,
            first_frame: 0,
            monotonic_ns: Some(0),
            discontinuity: false,
        })
        .unwrap();
    assert_eq!(mapped.first_frame, 0);
    assert_eq!(mapped.frames, 512);
    assert_eq!(peer.failure(), None);
    assert!(
        peer.output[..128]
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
}
