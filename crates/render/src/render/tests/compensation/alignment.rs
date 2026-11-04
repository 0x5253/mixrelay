//! Dry and wet paths arrive together, and unknown latency claims no compensation.
use super::*;

#[test]
fn dry_and_wet_impulses_align_across_variable_partitions_and_channel_widths() {
    for channels in [2_u16, 32] {
        let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
        let execution = diamond_execution(
            1,
            channels,
            128,
            false,
            [NodeControl::default(), NodeControl::default()],
        );
        let mut processor = prepare_processor(&execution, &factory);
        let output = render_impulse_stream(&mut processor, usize::from(channels), 0, 1.0);

        assert_eq!(processor.compensation_state(), CompensationState::Current);
        assert_aligned_impulse(&output, usize::from(channels), TEST_DELAY as usize, 2.0);
    }
}

#[test]
fn unknown_host_latency_does_not_claim_or_apply_false_compensation() {
    let factory = DelayFactory::new(TEST_DELAY, None);
    let execution = diamond_execution(
        1,
        2,
        128,
        false,
        [NodeControl::default(), NodeControl::default()],
    );
    let mut processor = prepare_processor(&execution, &factory);
    let output = render_impulse_stream(&mut processor, 2, 0, 1.0);

    assert_eq!(processor.compensation_state(), CompensationState::Unknown);
    for frame in 0..PARTITIONS.iter().sum::<usize>() {
        for channel in 0..2 {
            let expected = if frame == 0 || frame == TEST_DELAY as usize {
                f32::from(u16::try_from(channel + 1).expect("test width fits u16"))
            } else {
                0.0
            };
            assert_eq!(output[frame * 2 + channel].to_bits(), expected.to_bits());
        }
    }
}

fn measure_delay_case(channels: u16, compensation: bool) -> serde_json::Value {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(1, channels, 32, false, [NodeControl::default(); 2]);
    let mut processor = prepare_processor(&execution, &factory);
    if !compensation {
        processor.delay_bank = None;
    }
    let input = vec![0.01; usize::from(channels) * 32];
    let mut output = vec![0.0; input.len()];
    for _ in 0..500 {
        processor
            .process_block(32, &[&input], &mut [&mut output])
            .unwrap();
    }
    let mut timings = vec![0_u64; 3000];
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    for timing in &mut timings {
        let started = std::time::Instant::now();
        processor
            .process_block(
                32,
                &[std::hint::black_box(&input)],
                &mut [std::hint::black_box(&mut output)],
            )
            .unwrap();
        *timing = u64::try_from(started.elapsed().as_nanos()).unwrap();
        std::hint::black_box(&output);
    }
    let heap_operations = HEAP_OPERATIONS
        .with(|counter| counter.replace(None))
        .unwrap();
    timings.sort_unstable();
    assert_eq!(heap_operations, 0);
    serde_json::json!({
        "channels": channels, "frames": 32, "compensation": compensation,
        "p50_ns": timings[1500], "p95_ns": timings[2850], "p99_ns": timings[2970],
        "max_ns": timings[2999], "heap_operations": heap_operations,
        "over_budget": timings.iter().filter(|&&n| n > 666_666).count(),
    })
}

#[test]
#[ignore = "serial comparative benchmark; execute explicitly in release with no other workloads"]
fn measure_delayed_diamond_render() {
    for channels in [2, 32, 64] {
        for compensation in [false, true] {
            println!("{}", measure_delay_case(channels, compensation));
        }
    }
}
