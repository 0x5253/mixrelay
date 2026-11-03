//! Device-bus history, discontinuity scope and callback partition invariance.
use super::*;

#[test]
fn device_snapshot_never_replays_pcm_from_a_discontinued_input_timeline() {
    for (clock_restart, source_restart) in [(false, false), (true, false), (false, true)] {
        let execution = compile(
            vec![
                device_input("in"),
                node("bus", NodeConfig::DeviceBus { channels: 2 }),
                device_output("out"),
            ],
            vec![
                edge("publish", "in", AUDIO_PORT_ID, "bus", AUDIO_PORT_ID),
                edge("read", "bus", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
            ],
        );
        let mut processor =
            BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap();
        let (_, mut io, mut cycle) = callback_fixture();
        io.value = 1.0;
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
        cycle.first_frame += 4;
        if clock_restart {
            cycle.clock.epoch += 1;
            cycle.discontinuity = true;
        }
        io.value = 0.0;
        io.discontinuity = source_restart;
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        let expected = if clock_restart || source_restart {
            0.0_f32
        } else {
            1.0
        };
        assert_eq!(io.output.map(f32::to_bits), [expected; 8].map(f32::to_bits));
    }
}

#[test]
fn device_bus_preserves_pcm_across_unequal_callback_lengths() {
    for channels in [1, 2, 8, 32, 64] {
        let mut processor = device_bus_processor(channels);
        let channels = usize::from(channels);
        let mut input = vec![0.0; 4 * channels];
        let mut output = vec![0.0; input.len()];
        let mut position = 0;
        let tag = |sample| f32::from(u16::try_from(sample % 16_384 + 1).unwrap());
        for frames in [1, 2, 4, 1, 3, 2, 2, 1, 4].into_iter().cycle().take(128) {
            for (sample, value) in input.iter_mut().enumerate() {
                *value = tag(position * channels + sample);
            }
            HEAP_OPERATIONS.with(|count| count.set(Some(0)));
            processor
                .process_block(frames, &[&input], &mut [&mut output])
                .unwrap();
            assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
            for (sample, actual) in output.iter().enumerate().take(frames as usize * channels) {
                let absolute = position * channels + sample;
                let expected = absolute.checked_sub(4 * channels).map_or(0.0, tag);
                assert_eq!(
                    actual.to_bits(),
                    expected.to_bits(),
                    "channels={channels}, position={position}, sample={sample}"
                );
            }
            position += frames as usize;
        }
    }
}

#[test]
fn device_bus_replacement_retains_unread_tail_and_ring_position() {
    let mut previous = device_bus_processor(1);
    let mut output = [0.0; 4];
    previous
        .process_block(4, &[&[1.0, 2.0, 3.0, 4.0]], &mut [&mut output])
        .unwrap();
    previous
        .process_block(1, &[&[5.0]], &mut [&mut output])
        .unwrap();
    assert_eq!(output[0].to_bits(), 1.0_f32.to_bits());
    let mut next = device_bus_processor(1);
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    next.retain_device_bus_state(&previous);
    next.process_block(4, &[&[6.0, 7.0, 8.0, 9.0]], &mut [&mut output])
        .unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(
        output.map(f32::to_bits),
        [2.0_f32, 3.0, 4.0, 5.0].map(f32::to_bits)
    );
}

#[test]
fn one_source_discontinuity_does_not_clear_unrelated_device_bus_history() {
    let (mut processor, mut io, mut cycle) = independent_bus_fixture();
    processor.process_external_cycle(cycle, &mut io).unwrap();
    cycle.first_frame += u64::from(cycle.frames);
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(io.output_a.map(f32::to_bits), [1.0_f32.to_bits(); 8]);
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);

    cycle.first_frame += u64::from(cycle.frames);
    io.source_a_discontinuity = true;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    let report = processor
        .process_external_cycle(cycle, &mut io)
        .unwrap()
        .unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(report.discontinuities, 1);
    assert_eq!(io.output_a.map(f32::to_bits), [0.0_f32.to_bits(); 8]);
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
}

#[test]
fn lossless_clock_handoff_preserves_unrelated_bus_history_without_allocation() {
    for source_a_discontinuity in [false, true] {
        let (mut processor, mut io, mut cycle) = independent_bus_fixture();
        processor.process_external_cycle(cycle, &mut io).unwrap();
        cycle.first_frame += u64::from(cycle.frames);
        processor.process_external_cycle(cycle, &mut io).unwrap();

        io.source_a_discontinuity = source_a_discontinuity;
        io.admission = CycleAdmission::ClockHandoff;
        cycle.clock.id = 2;
        cycle.first_frame = 960;
        cycle.discontinuity = true;
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
        let expected_a = if source_a_discontinuity {
            0.0_f32
        } else {
            1.0_f32
        };
        assert_eq!(io.output_a.map(f32::to_bits), [expected_a.to_bits(); 8]);

        // Retaining PCM history must still commit the new clock coordinates.
        io.admission = CycleAdmission::Process;
        io.source_a_discontinuity = false;
        cycle.first_frame += u64::from(cycle.frames);
        cycle.discontinuity = false;
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
    }
}

#[test]
fn global_clock_discontinuity_clears_both_independent_bus_snapshots() {
    let (mut processor, mut io, mut cycle) = independent_bus_fixture();
    processor.process_external_cycle(cycle, &mut io).unwrap();
    cycle.first_frame += u64::from(cycle.frames);
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(io.output_a.map(f32::to_bits), [1.0_f32.to_bits(); 8]);
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);

    cycle.first_frame += u64::from(cycle.frames);
    cycle.clock.epoch += 1;
    cycle.discontinuity = true;
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(io.output_a.map(f32::to_bits), [0.0_f32.to_bits(); 8]);
    assert_eq!(io.output_b.map(f32::to_bits), [0.0_f32.to_bits(); 8]);
}

#[test]
fn local_source_restart_retains_other_bus_partial_quantum_position() {
    let (mut processor, mut io, mut cycle) = independent_bus_fixture();
    io.source_b = [2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 5.0, 5.0];
    processor.process_external_cycle(cycle, &mut io).unwrap();
    cycle.first_frame += 4;
    cycle.frames = 2;
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(
        io.output_b.map(f32::to_bits)[..4],
        [2.0_f32, 2.0, 3.0, 3.0].map(f32::to_bits)
    );

    cycle.first_frame += 2;
    io.source_a_discontinuity = true;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(
        io.output_b.map(f32::to_bits)[..4],
        [4.0_f32, 4.0, 5.0, 5.0].map(f32::to_bits)
    );
}

#[test]
fn source_history_scope_follows_causal_cycles_without_reaching_other_branches() {
    let execution = compile(
        vec![
            device_input("source-a"),
            device_input("source-b"),
            node("bus-a", NodeConfig::DeviceBus { channels: 2 }),
            node("cycle-a", NodeConfig::DeviceBus { channels: 2 }),
            node("bus-b", NodeConfig::DeviceBus { channels: 2 }),
        ],
        vec![
            edge("a", "source-a", AUDIO_PORT_ID, "bus-a", AUDIO_PORT_ID),
            edge("forward", "bus-a", AUDIO_PORT_ID, "cycle-a", AUDIO_PORT_ID),
            edge("feedback", "cycle-a", AUDIO_PORT_ID, "bus-a", AUDIO_PORT_ID),
            edge("b", "source-b", AUDIO_PORT_ID, "bus-b", AUDIO_PORT_ID),
        ],
    );
    let plan = RenderPlan::prepare(&execution).unwrap();
    for (binding, targets) in plan.source_bindings.iter().zip(&plan.source_history) {
        let expected = match binding.node_id().as_str() {
            "source-a" => (2, 3),
            "source-b" => (1, 1),
            other => panic!("unexpected source {other}"),
        };
        assert_eq!(targets.device_buses.len(), expected.0);
        assert_eq!(targets.delay_routes.len(), expected.1);
    }
}

#[test]
fn source_history_does_not_cross_independent_duplex_transport_directions() {
    let execution = compile(
        vec![
            device_input("source-a"),
            virtual_endpoint("duplex-b"),
            node("bus-b", NodeConfig::DeviceBus { channels: 2 }),
            device_output("sink-b"),
        ],
        vec![
            edge(
                "a-sink",
                "source-a",
                AUDIO_PORT_ID,
                "duplex-b",
                TO_OS_PORT_ID,
            ),
            edge(
                "b-source",
                "duplex-b",
                FROM_OS_PORT_ID,
                "bus-b",
                AUDIO_PORT_ID,
            ),
            edge("b-sink", "bus-b", AUDIO_PORT_ID, "sink-b", AUDIO_PORT_ID),
        ],
    );
    let plan = RenderPlan::prepare(&execution).unwrap();
    for (binding, targets) in plan.source_bindings.iter().zip(&plan.source_history) {
        let expected = match binding.node_id().as_str() {
            "source-a" => (0, 1),
            "duplex-b" => (1, 2),
            other => panic!("unexpected source {other}"),
        };
        assert_eq!(targets.device_buses.len(), expected.0);
        assert_eq!(targets.delay_routes.len(), expected.1);
    }
}

#[test]
fn device_bus_has_one_prepared_quantum_latency_without_delaying_direct_hardware() {
    let nodes = vec![
        device_input("mic"),
        device_output("out"),
        node("bus", NodeConfig::DeviceBus { channels: 2 }),
        node(
            "gain",
            NodeConfig::Gain(GainConfig {
                channels: 2,
                gain_db: -6.020_600_3,
            }),
        ),
    ];
    let edges = vec![
        edge("publish", "mic", AUDIO_PORT_ID, "bus", AUDIO_PORT_ID),
        edge("snapshot", "bus", AUDIO_PORT_ID, "gain", AUDIO_PORT_ID),
        edge("direct", "mic", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        edge("read", "gain", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
    ];
    let plan = Arc::new(RenderPlan::prepare(&compile(nodes, edges)).unwrap());
    let mut processor = BlockProcessor::new(plan).unwrap();
    let first = [0.2, 0.4, 0.6, 0.8, 0.1, 0.3, 0.5, 0.7];
    let second = [1.0; 8];
    let mut output = [0.0; 8];
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor
        .process_block(4, &[&first], &mut [&mut output])
        .unwrap();
    assert_eq!(output.map(f32::to_bits), first.map(f32::to_bits));
    processor
        .process_block(1, &[&second], &mut [&mut output])
        .unwrap();
    assert!((output[0] - 1.1).abs() < 0.000_001);
    assert!((output[1] - 1.2).abs() < 0.000_001);
    processor
        .process_block(3, &[&second], &mut [&mut output])
        .unwrap();
    for (actual, expected) in output.iter().zip([1.3, 1.4, 1.05, 1.15, 1.25, 1.35]) {
        assert!((actual - expected).abs() < 0.000_001);
    }
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
}
