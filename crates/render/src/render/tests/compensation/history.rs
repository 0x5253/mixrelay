//! Compensation history across bank refresh, topology replacement, Off, Mute and restarts.
use super::*;

#[test]
fn bank_refresh_preserves_warmed_history_and_changes_latency_without_reprepare() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(
        1,
        2,
        128,
        false,
        [NodeControl::default(), NodeControl::default()],
    );
    let mut processor = Box::new(prepare_processor(&execution, &factory));
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);
    exchange.observe_plan(Arc::clone(&plan));

    let mut first_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[1.0, 1.0], &mut first_output);
    assert_eq!(
        first_output.map(f32::to_bits),
        [0.0_f32; 2].map(f32::to_bits)
    );
    assert!(exchange.refresh_latency().expect("initial bank refresh"));

    let revisions = exchange.plan_revisions();
    let mut native_activations = 0;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    let activated = exchange
        .activate_pending_configured(&mut processor, |_| {
            native_activations += 1;
            Ok::<(), String>(())
        })
        .expect("bank-only activation should not run native setup");
    let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
    assert_eq!(operations, Some(0));
    assert!(!activated);
    assert_eq!(native_activations, 0);
    assert_eq!(exchange.plan_revisions(), revisions);
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(factory.counts().prepares.load(Ordering::Relaxed), 1);

    let mut tail_output = [0.0; 8];
    render_without_heap(&mut processor, 4, &[0.0; 8], &mut tail_output);
    assert_eq!(
        tail_output.map(f32::to_bits),
        [0.0_f32; 8].map(f32::to_bits)
    );
    let mut impulse_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[0.0; 2], &mut impulse_output);
    assert_eq!(
        impulse_output.map(f32::to_bits),
        [2.0_f32; 2].map(f32::to_bits)
    );

    factory.state.actual_frames.store(7, Ordering::Release);
    factory.state.reported_frames.store(7, Ordering::Release);
    let mut pending_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[0.0; 2], &mut pending_output);
    assert_eq!(processor.compensation_state(), CompensationState::Pending);
    assert!(exchange.refresh_latency().expect("changed latency refresh"));
    assert!(!exchange.activate_pending(&mut processor));
    exchange.reclaim_retired();

    let updated = render_impulse_stream(&mut processor, 2, 0, 1.0);
    assert_eq!(processor.compensation_state(), CompensationState::Current);
    assert_aligned_impulse(&updated, 2, 7, 2.0);
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(exchange.plan_revisions(), revisions);
    assert_eq!(factory.counts().prepares.load(Ordering::Relaxed), 1);
    assert_eq!(factory.counts().active_changes.load(Ordering::Relaxed), 0);
    assert!(factory.counts().calls.load(Ordering::Relaxed) > 1);
}

#[test]
fn stale_bank_for_replaced_topology_cannot_apply_to_the_new_plan() {
    let factory = DelayFactory::new(3, Some(3));
    let old_execution = diamond_execution(1, 2, 128, false, [NodeControl::default(); 2]);
    let mut current = Box::new(prepare_processor(&old_execution, &factory));
    let exchange = crate::ProcessorExchange::new(1);
    exchange.observe_plan(Arc::clone(current.plan()));

    factory.state.actual_frames.store(7, Ordering::Release);
    factory.state.reported_frames.store(7, Ordering::Release);
    assert!(
        exchange
            .refresh_latency()
            .expect("old topology bank refresh")
    );

    factory.state.actual_frames.store(2, Ordering::Release);
    factory.state.reported_frames.store(2, Ordering::Release);
    let new_execution = diamond_execution(
        2,
        2,
        128,
        true,
        [NodeControl::default(), NodeControl::default()],
    );
    let replacement = Box::new(prepare_processor(&new_execution, &factory));
    let replacement_plan = Arc::clone(replacement.plan());
    exchange
        .submit(replacement)
        .expect("replacement topology should queue");
    assert!(exchange.activate_pending(&mut current));
    exchange.reclaim_retired();

    assert!(Arc::ptr_eq(current.plan(), &replacement_plan));
    assert_eq!(current.plan().revision(), 2);
    assert_eq!(current.compensation_state(), CompensationState::Current);
    assert_eq!(exchange.plan_revisions().active, 2);
    assert_eq!(exchange.plan_revisions().submitted, 2);
    assert!(!exchange.activate_pending(&mut current));

    let output = render_impulse_stream(&mut current, 2, 0, 1.0);
    assert_aligned_impulse(&output, 2, 2, 2.0);
    assert_eq!(factory.counts().prepares.load(Ordering::Relaxed), 2);
}

#[test]
fn destination_off_and_mute_discard_its_queued_compensation_history() {
    for transition in [
        NodeControl {
            enabled: false,
            muted: false,
        },
        NodeControl {
            enabled: true,
            muted: true,
        },
    ] {
        let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
        let execution = diamond_execution(
            1,
            2,
            128,
            false,
            [
                NodeControl {
                    enabled: true,
                    muted: true,
                },
                NodeControl::default(),
            ],
        );
        let mut processor = prepare_processor(&execution, &factory);
        let plan = Arc::clone(processor.plan());
        let exchange = crate::ProcessorExchange::new(1);
        let mut first = [0.0; 2];
        render_without_heap(&mut processor, 1, &[1.0; 2], &mut first);

        publish_control(&exchange, &plan, 2, "mix", transition);
        let mut transition_output = [0.0; 2];
        render_without_heap(&mut processor, 1, &[0.0; 2], &mut transition_output);
        assert_eq!(
            transition_output.map(f32::to_bits),
            [0.0_f32; 2].map(f32::to_bits)
        );

        publish_control(&exchange, &plan, 3, "mix", NodeControl::default());
        let mut resumed = [0.0; 10];
        render_without_heap(&mut processor, 5, &[0.0; 10], &mut resumed);
        assert_eq!(resumed.map(f32::to_bits), [0.0_f32; 10].map(f32::to_bits));
    }
}

#[test]
fn input_timeline_restart_discards_queued_route_history_without_heap_work() {
    for (clock_restart, source_restart) in [(false, false), (true, false), (false, true)] {
        let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
        let execution = diamond_execution(
            1,
            2,
            128,
            false,
            [
                NodeControl {
                    enabled: true,
                    muted: true,
                },
                NodeControl::default(),
            ],
        );
        let mut processor = prepare_processor(&execution, &factory);
        let (_, mut io, mut cycle) = callback_fixture();
        io.value = 1.0;
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));

        io.value = 0.0;
        io.discontinuity = source_restart;
        cycle.first_frame += 4;
        if clock_restart {
            cycle.clock.epoch += 1;
            cycle.discontinuity = true;
        }
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        let result = processor.process_external_cycle(cycle, &mut io);
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        result.unwrap();
        let expected = if clock_restart || source_restart {
            [0.0_f32; 8]
        } else {
            [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]
        };
        assert_eq!(io.output.map(f32::to_bits), expected.map(f32::to_bits));
        assert_eq!(processor.compensation_state(), CompensationState::Current);
    }
}

#[test]
fn unrelated_source_restart_retains_compensated_route_history() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let mixer = node(
        "mix",
        NodeConfig::Mixer(MixerConfig {
            inputs: vec![PortId::from("dry"), PortId::from("wet")],
            channels: 2,
        }),
    );
    let execution = GraphCompiler::new(DelayProviderResolver)
        .compile(
            &GraphDocument::new(
                1,
                vec![
                    device_input("source-a"),
                    device_input("source-b"),
                    device_output("sink-a"),
                    device_output("sink-b"),
                    provider_node(
                        2,
                        NodeControl {
                            enabled: true,
                            muted: true,
                        },
                    ),
                    mixer,
                ],
                vec![
                    edge("a", "source-a", AUDIO_PORT_ID, "sink-a", AUDIO_PORT_ID),
                    edge("b-wet", "source-b", AUDIO_PORT_ID, "effect", AUDIO_PORT_ID),
                    edge("wet", "effect", AUDIO_PORT_ID, "mix", "wet"),
                    edge("b-dry", "source-b", AUDIO_PORT_ID, "mix", "dry"),
                    edge("out", "mix", AUDIO_PORT_ID, "sink-b", AUDIO_PORT_ID),
                ],
            ),
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    let mut processor = prepare_processor(&execution, &factory);
    let (_, mut io, mut cycle) = independent_bus_fixture();
    for _ in 0..3 {
        processor.process_external_cycle(cycle, &mut io).unwrap();
        cycle.first_frame += u64::from(cycle.frames);
    }
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
    io.source_a_discontinuity = true;
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    assert_eq!(io.output_b.map(f32::to_bits), [2.0_f32.to_bits(); 8]);
}

#[test]
fn hosted_off_suspends_its_own_delay_history_before_reenable() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = linear_execution(1, 2, 128, NodeControl::default());
    let mut processor = prepare_processor(&execution, &factory);
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);

    let mut first = [0.0; 2];
    render_without_heap(&mut processor, 1, &[1.0; 2], &mut first);
    publish_control(
        &exchange,
        &plan,
        2,
        "effect",
        NodeControl {
            enabled: false,
            muted: false,
        },
    );
    let mut off_output = [0.0; 2];
    render_without_heap(&mut processor, 1, &[0.0; 2], &mut off_output);
    publish_control(&exchange, &plan, 3, "effect", NodeControl::default());

    let mut resumed = [0.0; 12];
    render_without_heap(&mut processor, 6, &[0.0; 12], &mut resumed);
    assert_eq!(resumed.map(f32::to_bits), [0.0_f32; 12].map(f32::to_bits));
    assert_eq!(factory.counts().suspensions.load(Ordering::Relaxed), 1);
}
