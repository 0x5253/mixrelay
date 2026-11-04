//! Measured arrivals include compensation and stay scoped to their own path.
use super::*;

#[test]
fn destination_arrival_includes_compensation_and_rejects_unmeasured_sources() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(1, 1, 32, false, [NodeControl::default(); 2]);
    let processor = prepare_processor(&execution, &factory);
    let plan = processor.plan();
    assert_eq!(
        plan.destination_arrivals(&[Some(0)]).unwrap(),
        vec![Some(TEST_DELAY)]
    );
    assert_eq!(
        plan.destination_arrivals(&[Some(10)]).unwrap(),
        vec![Some(TEST_DELAY + 10)]
    );
    assert_eq!(plan.destination_arrivals(&[None]).unwrap(), vec![None]);
    assert!(plan.destination_arrivals(&[]).is_err());
}

#[test]
fn snapshot_content_arrival_matches_pcm_delay_across_short_callbacks() {
    let execution = compile(
        vec![
            virtual_endpoint("A/native"),
            node("A/snapshot", NodeConfig::DeviceBus { channels: 2 }),
            node(
                "gain",
                NodeConfig::Gain(GainConfig {
                    channels: 2,
                    gain_db: 0.0,
                }),
            ),
            virtual_endpoint("D/native"),
        ],
        vec![
            edge(
                "publish",
                "A/native",
                FROM_OS_PORT_ID,
                "A/snapshot",
                AUDIO_PORT_ID,
            ),
            edge("read", "A/snapshot", AUDIO_PORT_ID, "gain", AUDIO_PORT_ID),
            edge("out", "gain", AUDIO_PORT_ID, "D/native", TO_OS_PORT_ID),
        ],
    );
    let mut processor = prepare_processor(&execution, &DelayFactory::new(0, Some(0)));
    let mut stream = Vec::new();
    for (tick, frames) in [1, 2, 1, 3, 1].into_iter().enumerate() {
        let mut source = [0.0; 8];
        if tick == 0 {
            source[..2].copy_from_slice(&[1.0, 2.0]);
        }
        let mut a = [0.0; 8];
        let mut d = [0.0; 8];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor
            .process_block(frames, &[&source, &[0.0; 8]], &mut [&mut a, &mut d])
            .unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        stream.extend_from_slice(&d[..frames as usize * 2]);
    }
    assert!(processor.acknowledge_content_latency());
    let plan = processor.plan();
    assert_eq!(plan.destination_source_indexes(), vec![vec![], vec![0]]);
    let root = 1_000_000.25;
    let arrival = root + f64::from(FORMAT.block_frames) * 1e9 / 48_000.0;
    assert_eq!(
        plan.measured_destination_arrivals(&[Some(root), None], 1e6)
            .unwrap(),
        vec![None, Some(arrival)]
    );
    assert_eq!(
        plan.measured_destination_arrivals(&[None, None], 1e6)
            .unwrap(),
        vec![None, None]
    );
    assert_aligned_impulse(&stream, 2, FORMAT.block_frames as usize, 1.0);
}

#[test]
fn measured_arrivals_require_acknowledged_hosted_history_and_preserve_fractional_roots() {
    let factory = DelayFactory::new(TEST_DELAY, Some(TEST_DELAY));
    let execution = diamond_execution(1, 1, 32, false, [NodeControl::default(); 2]);
    let mut processor = prepare_processor(&execution, &factory);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![None]
    );
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    assert!(!processor.acknowledge_content_latency());
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    let delay = f64::from(TEST_DELAY) * 1e9 / 48_000.0;
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![Some(delay + 0.25)]
    );
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[None], 1e6)
            .unwrap(),
        vec![None]
    );
    // A read that keeps overlapping the audio thread is disturbed, not unknown.
    processor.plan().compensation.acknowledging(true);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6),
        Err(crate::ArrivalError::Contended)
    );
    processor.plan().compensation.acknowledging(false);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![Some(delay + 0.25)]
    );
    assert!(matches!(
        processor.plan().measured_destination_arrivals(&[], 1e6),
        Err(crate::ArrivalError::Invalid(_))
    ));
    assert_eq!(processor.plan().destination_source_indexes(), vec![vec![0]]);
    render_without_heap(&mut processor, 32, &[0.0; 32], &mut [0.0; 32]);
    assert!(
        !processor.acknowledge_content_latency(),
        "a partial warmup block still taints its window"
    );
    render_without_heap(&mut processor, 32, &[0.0; 32], &mut [0.0; 32]);
    assert!(processor.acknowledge_content_latency());
    factory
        .state
        .reported_frames
        .store(TEST_DELAY + 1, Ordering::Release);
    assert_eq!(
        processor
            .plan()
            .measured_destination_arrivals(&[Some(0.25)], 1e6)
            .unwrap(),
        vec![None]
    );
    processor.delay_bank.as_mut().unwrap().validate();
    assert!(!processor.acknowledge_content_latency());
}

struct PathIo {
    input_bad: bool,
    output_bad: bool,
    discontinuity: bool,
}

impl ExternalIo for PathIo {
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        _: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        TransferOutcome {
            frames: if self.input_bad && binding.node_id().as_str() == "bad" {
                0
            } else {
                cycle.frames
            },
            discontinuity: self.discontinuity && binding.node_id().as_str() == "bad",
            activity: mixrelay_audio_io::process::InputActivity::Active,
            source_sample_faults: 0,
        }
    }
    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        _: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        TransferOutcome {
            frames: if self.output_bad && binding.node_id().as_str() == "bad" {
                0
            } else {
                cycle.frames
            },
            discontinuity: self.discontinuity && binding.node_id().as_str() == "bad",
            activity: mixrelay_audio_io::process::InputActivity::Active,
            source_sample_faults: 0,
        }
    }
}

#[test]
fn binding_latency_faults_and_history_warmup_are_scoped_to_their_path() {
    let execution = compile(
        vec![virtual_endpoint("good"), virtual_endpoint("bad")],
        vec![
            edge("good-path", "good", FROM_OS_PORT_ID, "good", TO_OS_PORT_ID),
            edge("bad-path", "bad", FROM_OS_PORT_ID, "bad", TO_OS_PORT_ID),
        ],
    );
    let mut processor = prepare_processor(&execution, &DelayFactory::new(0, Some(0)));
    let (_, _, cycle) = callback_fixture();
    for (index, (input_bad, output_bad, discontinuity)) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (false, false, false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut io = PathIo {
            input_bad,
            output_bad,
            discontinuity,
        };
        let report = processor
            .process_external_cycle(
                ProcessCycle {
                    first_frame: index as u64 * 4,
                    ..cycle
                },
                &mut io,
            )
            .unwrap()
            .unwrap();
        assert_eq!(report.input_starvations, u64::from(input_bad));
        assert_eq!(report.output_overflows, u64::from(output_bad));
        assert_eq!(report.discontinuities, u64::from(discontinuity) * 2);
        let _ = processor.acknowledge_content_latency();
        assert!(processor.binding_content_latency_ready("good/capture", true));
        assert!(processor.binding_content_latency_ready("good/render", false));
        assert_eq!(
            processor.binding_content_latency_ready("bad", true),
            !input_bad && !discontinuity
        );
        assert_eq!(
            processor.binding_content_latency_ready("bad", false),
            !output_bad && !discontinuity
        );
        assert!(!processor.binding_content_latency_ready("missing", true));
    }
    // Independent route histories can be unprimed without tainting the good path.
    let bad = processor
        .plan
        .compensation
        .binding_routes(&NodeId::from("bad"));
    processor.delay_bank.as_mut().unwrap().reset_routes(&bad);
    assert!(processor.binding_content_latency_ready("good", false));
    assert!(!processor.binding_content_latency_ready("bad", false));
}
