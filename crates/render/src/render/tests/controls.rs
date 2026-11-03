use super::*;
use mixrelay_audio_graph::NodeControl;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Counts {
    prepares: AtomicUsize,
    calls: AtomicUsize,
}

struct CountedProcessor(Arc<Counts>);

impl HostedAudioProcessor for CountedProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        DoubleProcessor.process(input, output, frames)
    }
}

struct CountedFactory(Arc<Counts>);

impl HostedProcessorFactory for CountedFactory {
    fn prepare(
        &self,
        _: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        self.0.prepares.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(CountedProcessor(Arc::clone(&self.0))))
    }
}

fn provider_document(control: NodeControl) -> GraphDocument {
    let mut provider = node(
        "effect",
        NodeConfig::Provider(ProviderNodeConfig {
            type_id: mixrelay_audio_graph::NodeTypeId::from("test:double"),
            parameters: serde_json::json!({}),
            ports: None,
        }),
    );
    provider.control = control;
    GraphDocument::new(
        1,
        vec![device_input("in"), provider, device_output("out")],
        vec![
            edge("input", "in", AUDIO_PORT_ID, "effect", AUDIO_PORT_ID),
            edge("output", "effect", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    )
}

fn prepare_provider(document: &GraphDocument, counts: &Arc<Counts>) -> BlockProcessor {
    let execution = GraphCompiler::new(TestProviderResolver)
        .compile(
            document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(crate::PeakMeterBank::new(8, 8)),
        &CountedFactory(Arc::clone(counts)),
    )
    .unwrap()
}

#[test]
fn processor_off_avoids_factory_and_dsp_while_mute_keeps_processing_without_bypass() {
    for (control, expected_prepares, expected_calls, expected_sample) in [
        (NodeControl::default(), 1, 100, 2.0_f32),
        (
            NodeControl {
                enabled: false,
                muted: false,
            },
            0,
            0,
            0.0,
        ),
        (
            NodeControl {
                enabled: true,
                muted: true,
            },
            1,
            100,
            0.0,
        ),
    ] {
        let counts = Arc::new(Counts::default());
        let mut processor = prepare_provider(&provider_document(control), &counts);
        let mut output = [9.0; 8];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        for _ in 0..100 {
            processor
                .process_block(4, &[&[1.0; 8]], &mut [&mut output])
                .unwrap();
        }
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert_eq!(counts.prepares.load(Ordering::Relaxed), expected_prepares);
        assert_eq!(counts.calls.load(Ordering::Relaxed), expected_calls);
        assert_eq!(
            output.map(f32::to_bits),
            [expected_sample; 8].map(f32::to_bits)
        );
    }
}

#[test]
fn restoring_enabled_node_retains_authored_connections_and_parameters() {
    let counts = Arc::new(Counts::default());
    let document = provider_document(NodeControl::default());
    let saved = document.clone();
    let mut processor = prepare_provider(&document, &counts);
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);
    for (revision, enabled) in [(2, true), (3, false), (4, true)] {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("effect"),
                    control: NodeControl {
                        enabled,
                        muted: false,
                    },
                    gain_db: None,
                }],
            )
            .unwrap();
        let mut output = [9.0; 8];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor
            .process_block(4, &[&[1.0; 8]], &mut [&mut output])
            .unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        let expected = if enabled { 2.0_f32 } else { 0.0 };
        assert_eq!(output.map(f32::to_bits), [expected; 8].map(f32::to_bits));
    }
    assert_eq!(document, saved);
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(counts.prepares.load(Ordering::Relaxed), 1);
    assert_eq!(counts.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        exchange.plan_revisions(),
        crate::ProcessorRevisions {
            active: 1,
            submitted: 1
        }
    );
    assert_eq!(
        exchange.revisions(),
        crate::ProcessorRevisions {
            active: 4,
            submitted: 4
        }
    );
}

#[derive(Default)]
struct BufferedHistoryCounts {
    prepares: AtomicUsize,
    calls: AtomicUsize,
    suspensions: AtomicUsize,
}

struct BufferedHistoryProcessor {
    counts: Arc<BufferedHistoryCounts>,
    history: [f32; 8],
}

impl HostedAudioProcessor for BufferedHistoryProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool {
        self.counts.calls.fetch_add(1, Ordering::Relaxed);
        let samples = frames * 2;
        output[..samples].copy_from_slice(&self.history[..samples]);
        self.history[..samples].copy_from_slice(&input[..samples]);
        true
    }

    fn suspend_processing(&mut self) {
        self.counts.suspensions.fetch_add(1, Ordering::Relaxed);
        self.history.fill(0.0);
    }
}

struct BufferedHistoryFactory(Arc<BufferedHistoryCounts>);

impl HostedProcessorFactory for BufferedHistoryFactory {
    fn prepare(
        &self,
        _: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        self.0.prepares.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(BufferedHistoryProcessor {
            counts: Arc::clone(&self.0),
            history: [0.0; 8],
        }))
    }
}

fn process_without_heap(processor: &mut BlockProcessor, input: &[f32; 8], output: &mut [f32; 8]) {
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor.process_block(4, &[input], &mut [output]).unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
}

fn process_with_expected_output(processor: &mut BlockProcessor, input: &[f32; 8], expected: f32) {
    let mut output = [f32::NAN; 8];
    process_without_heap(processor, input, &mut output);
    assert_eq!(output.map(f32::to_bits), [expected; 8].map(f32::to_bits));
}

fn publish_provider_control(
    exchange: &crate::ProcessorExchange,
    plan: &RenderPlan,
    revision: u64,
    control: NodeControl,
) {
    exchange
        .publish_controls(
            plan.controls(),
            revision,
            &[crate::RuntimeControlUpdate {
                node_id: NodeId::from("effect"),
                control,
                gain_db: None,
            }],
        )
        .unwrap();
}

#[test]
fn processor_off_suspends_buffered_history_but_mute_keeps_processing() {
    let counts = Arc::new(BufferedHistoryCounts::default());
    let document = provider_document(NodeControl::default());
    let execution = GraphCompiler::new(TestProviderResolver)
        .compile(
            &document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    let mut processor = BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(crate::PeakMeterBank::new(8, 8)),
        &BufferedHistoryFactory(Arc::clone(&counts)),
    )
    .unwrap();
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);
    process_with_expected_output(&mut processor, &[1.0; 8], 0.0);
    process_with_expected_output(&mut processor, &[2.0; 8], 1.0);

    for revision in [2, 3] {
        publish_provider_control(
            &exchange,
            &plan,
            revision,
            NodeControl {
                enabled: false,
                muted: false,
            },
        );
        process_with_expected_output(&mut processor, &[3.0; 8], 0.0);
    }
    assert_eq!(counts.calls.load(Ordering::Relaxed), 2);
    assert_eq!(counts.suspensions.load(Ordering::Relaxed), 2);

    publish_provider_control(&exchange, &plan, 4, NodeControl::default());
    process_with_expected_output(&mut processor, &[4.0; 8], 0.0);

    publish_provider_control(
        &exchange,
        &plan,
        5,
        NodeControl {
            enabled: true,
            muted: true,
        },
    );
    process_with_expected_output(&mut processor, &[5.0; 8], 0.0);
    assert_eq!(counts.suspensions.load(Ordering::Relaxed), 2);

    publish_provider_control(&exchange, &plan, 6, NodeControl::default());
    process_with_expected_output(&mut processor, &[6.0; 8], 5.0);

    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(counts.prepares.load(Ordering::Relaxed), 1);
    assert_eq!(counts.calls.load(Ordering::Relaxed), 5);
    assert_eq!(counts.suspensions.load(Ordering::Relaxed), 2);
}

#[test]
fn disabled_source_retains_binding_but_does_not_affect_another_active_route() {
    let mut disabled = device_input("disabled");
    disabled.control.enabled = false;
    let execution = compile(
        vec![disabled, device_input("active"), device_output("out")],
        vec![
            edge("off", "disabled", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
            edge("on", "active", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let plan = RenderPlan::prepare(&execution).unwrap();
    assert_eq!(plan.source_bindings().len(), 2);
    assert_eq!(plan.source_bindings()[0].node_id(), &NodeId::from("active"));
    let mut processor = BlockProcessor::new(Arc::new(plan)).unwrap();
    let mut output = [0.0; 8];
    processor
        .process_block(4, &[&[0.5; 8], &[9.0; 8]], &mut [&mut output])
        .unwrap();
    assert_eq!(output.map(f32::to_bits), [0.5_f32; 8].map(f32::to_bits));
}

#[test]
fn source_and_sink_mute_preserve_native_bindings_but_silence_the_boundary() {
    for muted_index in [0, 1] {
        let mut nodes = vec![device_input("in"), device_output("out")];
        nodes[muted_index].control.muted = true;
        let execution = compile(
            nodes,
            vec![edge("route", "in", AUDIO_PORT_ID, "out", AUDIO_PORT_ID)],
        );
        let plan = RenderPlan::prepare(&execution).unwrap();
        assert_eq!(plan.source_bindings().len(), 1);
        assert_eq!(plan.sink_bindings().len(), 1);
        let mut processor = BlockProcessor::new(Arc::new(plan)).unwrap();
        let mut output = [9.0; 8];
        processor
            .process_block(4, &[&[1.0; 8]], &mut [&mut output])
            .unwrap();
        assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
    }
}

#[test]
fn runtime_gain_and_mute_publish_without_preparation_or_audio_heap_operations() {
    let execution = compile(
        vec![device_input("in"), device_output("out")],
        vec![edge("route", "in", AUDIO_PORT_ID, "out", AUDIO_PORT_ID)],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let mut processor = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let exchange = crate::ProcessorExchange::new(1);
    let mut output = [0.0; 8];
    for revision in 2..102 {
        let muted = revision % 2 == 0;
        let updates = [crate::RuntimeControlUpdate {
            node_id: NodeId::from("in"),
            control: NodeControl {
                enabled: true,
                muted,
            },
            gain_db: Some(-6.020_600_3),
        }];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        exchange
            .publish_controls(plan.controls(), revision, &updates)
            .unwrap();
        processor
            .process_block(4, &[&[1.0; 8]], &mut [&mut output])
            .unwrap();
        let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
        assert_eq!(operations, Some(0));
        let expected = if muted { 0.0_f32 } else { 0.5 };
        assert!(
            output
                .iter()
                .all(|sample| (*sample - expected).abs() < 0.000_001)
        );
        assert_eq!(exchange.revisions().active, revision);
    }
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(exchange.plan_revisions().submitted, 1);
}

#[test]
fn source_gain_retains_float_headroom_and_meter_peaks_across_unity_boost_and_mute() {
    let execution = compile(
        vec![device_input("in"), device_output("out")],
        vec![edge("route", "in", AUDIO_PORT_ID, "out", AUDIO_PORT_ID)],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let meters = Arc::new(crate::PeakMeterBank::new(plan.meter_bindings().len(), 2));
    let mut processor =
        BlockProcessor::with_peak_meters(Arc::clone(&plan), Arc::clone(&meters)).unwrap();
    let exchange = crate::ProcessorExchange::new(1);
    let input = [0.5, 1.0, -0.5, -1.0, 0.25, -0.75, -0.25, 0.75];
    assert_eq!(plan.meter_bindings().len(), 2);

    for (revision, gain_db, muted, scale) in [
        (2, 0.0, false, 1.0),
        (3, 6.020_600_3, false, 2.0),
        (4, 6.020_600_3, true, 0.0),
        (5, 0.0, false, 1.0),
    ] {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("in"),
                    control: NodeControl {
                        enabled: true,
                        muted,
                    },
                    gain_db: Some(gain_db),
                }],
            )
            .unwrap();
        let mut output = [f32::NAN; 8];
        process_without_heap(&mut processor, &input, &mut output);
        for (actual, source) in output.into_iter().zip(input) {
            assert!((actual - source * scale).abs() <= 0.000_001);
        }
        if revision == 3 {
            assert!((output[0] - 1.0).abs() <= 0.000_001);
            assert!((output[1] - 2.0).abs() <= 0.000_001);
            assert!((output[3] + 2.0).abs() <= 0.000_001);
        }
        for index in 0..plan.meter_bindings().len() {
            let peaks = meters.take_peaks(revision, index, 2);
            assert_eq!(peaks.len(), 2);
            for (actual, expected) in peaks.into_iter().zip([0.5 * scale, scale]) {
                assert!((actual - expected).abs() <= 0.000_001);
            }
        }
    }
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    assert_eq!(exchange.plan_revisions().submitted, 1);
}

#[test]
fn peak_meter_take_resets_consumed_peaks_before_following_silence() {
    let meters = crate::PeakMeterBank::new(2, 2);
    meters.record_interleaved(7, 0, 2, &[0.25, -0.75, -0.5, 0.125]);
    meters.record_interleaved(7, 1, 2, &[1.25, -2.0, -0.25, 0.5]);
    assert_eq!(meters.take_peaks(7, 0, 2), [0.5, 0.75]);
    assert_eq!(meters.take_peaks(7, 0, 2), [0.0, 0.0]);
    assert_eq!(meters.take_peaks(7, 1, 2), [1.25, 2.0]);

    for binding in 0..2 {
        meters.record_interleaved(7, binding, 2, &[0.0; 8]);
        assert_eq!(meters.take_peaks(7, binding, 2), [0.0, 0.0]);
        assert_eq!(meters.take_peaks(7, binding, 2), [0.0, 0.0]);
    }
    meters.record_interleaved(7, 0, 2, &[0.5, -1.0]);
    meters.record_interleaved(8, 0, 2, &[0.0; 4]);
    assert_eq!(meters.take_peaks(7, 0, 2), [0.0, 0.0]);
    assert_eq!(meters.take_peaks(8, 0, 2), [0.0, 0.0]);
}

#[test]
fn invalid_batch_and_initially_off_provider_never_partially_publish() {
    let counts = Arc::new(Counts::default());
    let processor = prepare_provider(
        &provider_document(NodeControl {
            enabled: false,
            muted: false,
        }),
        &counts,
    );
    let bank = processor.plan().controls();
    let exchange = crate::ProcessorExchange::new(1);
    let source = crate::RuntimeControlUpdate {
        node_id: NodeId::from("in"),
        control: NodeControl {
            enabled: false,
            muted: true,
        },
        gain_db: Some(-6.0),
    };
    let provider = crate::RuntimeControlUpdate {
        node_id: NodeId::from("effect"),
        control: NodeControl::default(),
        gain_db: None,
    };
    assert_eq!(
        exchange.publish_controls(bank, 2, &[source, provider]),
        Err(crate::RuntimeControlError::NeedsPreparation(NodeId::from(
            "effect"
        )))
    );
    assert_eq!(
        bank.snapshot(&NodeId::from("in")),
        Some((NodeControl::default(), 1.0))
    );
    assert_eq!(bank.published_revision(), 1);
    assert_eq!(counts.prepares.load(Ordering::Relaxed), 0);
}

#[test]
fn disabled_recorder_retains_prepared_tap_and_routes_for_in_place_activation() {
    let mut recorder = node("record", NodeConfig::Recorder { channels: 2 });
    recorder.control.enabled = false;
    let execution = compile(
        vec![device_input("in"), recorder],
        vec![edge("record", "in", AUDIO_PORT_ID, "record", AUDIO_PORT_ID)],
    );
    let plan = RenderPlan::prepare(&execution).unwrap();
    assert_eq!(plan.recorder_bindings.len(), 1);
    assert_eq!(plan.sink_routes.len(), 1);
}

fn prepare_dynamics(config: &NodeConfig, muted: bool, revision: u64) -> Box<BlockProcessor> {
    let mut effect = node("effect", config.clone());
    effect.control.muted = muted;
    let document = GraphDocument::new(
        revision,
        vec![device_input("in"), effect, device_output("out")],
        vec![
            edge("in", "in", AUDIO_PORT_ID, "effect", AUDIO_PORT_ID),
            edge("out", "effect", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let execution = GraphCompiler::new(BuiltInNodeResolver)
        .compile(
            &document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

#[test]
fn mute_publication_retains_running_dynamics_and_unmute_continues_the_envelope() {
    for config in [
        NodeConfig::Compressor(mixrelay_audio_graph::CompressorConfig {
            version: 1,
            bypass: false,
            channels: 2,
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 100.0,
            makeup_gain_db: 0.0,
        }),
        NodeConfig::Limiter(mixrelay_audio_graph::LimiterConfig {
            version: 1,
            bypass: false,
            channels: 2,
            ceiling_db: -6.0,
            release_ms: 100.0,
        }),
    ] {
        let mut current = prepare_dynamics(&config, false, 1);
        let mut reference = prepare_dynamics(&config, false, 1);
        let mut output = [0.0; 8];
        let mut expected = [0.0; 8];
        current
            .process_block(4, &[&[1.0; 8]], &mut [&mut output])
            .unwrap();
        reference
            .process_block(4, &[&[1.0; 8]], &mut [&mut expected])
            .unwrap();
        let exchange = crate::ProcessorExchange::new(1);
        for (revision, muted) in [(2, true), (3, false)] {
            exchange
                .submit(prepare_dynamics(&config, muted, revision))
                .unwrap();
            HEAP_OPERATIONS.with(|count| count.set(Some(0)));
            assert!(exchange.activate_pending(&mut current));
            assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
            assert_eq!(current.dynamics_envelopes, reference.dynamics_envelopes);
            current
                .process_block(4, &[&[0.5; 8]], &mut [&mut output])
                .unwrap();
            reference
                .process_block(4, &[&[0.5; 8]], &mut [&mut expected])
                .unwrap();
            assert_eq!(current.dynamics_envelopes, reference.dynamics_envelopes);
            if muted {
                assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
            } else {
                assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
            }
        }
    }
}

fn prepare_bus(revision: u64, muted: bool, extra_bus: bool) -> Box<BlockProcessor> {
    let mut unrelated = node(
        "unrelated",
        NodeConfig::Gain(GainConfig {
            channels: 2,
            gain_db: 0.0,
        }),
    );
    unrelated.control.muted = muted;
    let mut nodes = vec![
        device_input("in"),
        node("bus", NodeConfig::DeviceBus { channels: 2 }),
        device_output("out"),
        unrelated,
    ];
    if extra_bus {
        // Changes the snapshot arena offsets without changing the surviving bus identity.
        nodes.push(node("a-new-bus", NodeConfig::DeviceBus { channels: 2 }));
    }
    let document = GraphDocument::new(
        revision,
        nodes,
        vec![
            edge("publish", "in", AUDIO_PORT_ID, "bus", AUDIO_PORT_ID),
            edge("read", "bus", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let execution = GraphCompiler::new(BuiltInNodeResolver)
        .compile(
            &document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

#[test]
fn unrelated_mute_publication_retains_device_bus_history_without_allocation() {
    for extra_bus in [false, true] {
        let mut current = prepare_bus(1, false, false);
        let mut reference = prepare_bus(1, false, false);
        let (_, mut io, mut cycle) = callback_fixture();
        let (_, mut reference_io, _) = callback_fixture();
        io.value = 0.75;
        reference_io.value = 0.75;
        current.process_external_cycle(cycle, &mut io).unwrap();
        reference
            .process_external_cycle(cycle, &mut reference_io)
            .unwrap();
        let exchange = crate::ProcessorExchange::new(1);
        exchange.submit(prepare_bus(2, true, extra_bus)).unwrap();
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        assert!(exchange.activate_pending(&mut current));
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        cycle.first_frame += 4;
        io.value = 0.25;
        reference_io.value = 0.25;
        current.process_external_cycle(cycle, &mut io).unwrap();
        reference
            .process_external_cycle(cycle, &mut reference_io)
            .unwrap();
        assert_eq!(
            io.output.map(f32::to_bits),
            reference_io.output.map(f32::to_bits)
        );
        assert_eq!(io.output.map(f32::to_bits), [0.75_f32; 8].map(f32::to_bits));
        cycle.first_frame += 4;
        cycle.discontinuity = true;
        current.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
    }
}

#[test]
fn off_external_node_skips_both_io_calls_and_reenables_in_the_same_plan() {
    let (mut processor, mut io, mut cycle) = callback_fixture();
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);
    io.value = 0.75;
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!((io.reads, io.writes), (1, 1));
    for (revision, enabled) in [(2, false), (3, true)] {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("io"),
                    control: NodeControl {
                        enabled,
                        muted: false,
                    },
                    gain_db: None,
                }],
            )
            .unwrap();
        cycle.first_frame += 4;
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        let calls = if enabled { 2 } else { 1 };
        assert_eq!((io.reads, io.writes), (calls, calls));
    }
    assert!(Arc::ptr_eq(processor.plan(), &plan));
}

struct ObservedControlIo {
    inner: CallbackIo,
    enabled: bool,
    begin_calls: usize,
    admission: CycleAdmission,
}

impl ExternalIo for ObservedControlIo {
    fn begin_cycle(
        &mut self,
        _: &ProcessCycle,
        controls: ExternalControlState<'_>,
    ) -> CycleAdmission {
        self.begin_calls += 1;
        assert_eq!(controls.source_enabled(0), Some(self.enabled));
        assert_eq!(controls.sink_enabled(0), Some(self.enabled));
        assert_eq!(controls.source_enabled(1), None);
        assert_eq!(controls.sink_enabled(1), None);
        assert!(controls.sources().all(|(binding, enabled)| {
            !binding.node_id().as_str().is_empty() && enabled == self.enabled
        }));
        assert!(controls.sinks().all(|(binding, enabled)| {
            !binding.node_id().as_str().is_empty() && enabled == self.enabled
        }));
        self.admission
    }

    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        assert!(self.enabled && self.begin_calls > 0);
        self.inner.read_source(binding, destination, cycle)
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        assert!(self.enabled && self.begin_calls > 0);
        self.inner.write_sink(binding, source, cycle)
    }
}

#[test]
fn external_cycle_observes_the_same_gate_snapshot_before_any_transfer_without_allocation() {
    let (mut processor, io, mut cycle) = callback_fixture();
    let bank = Arc::clone(processor.plan().controls());
    let exchange = crate::ProcessorExchange::new(1);
    let mut io = ObservedControlIo {
        inner: io,
        enabled: true,
        begin_calls: 0,
        admission: CycleAdmission::Process,
    };
    for (revision, enabled) in [(2, false), (3, true)] {
        exchange
            .publish_controls(
                &bank,
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("io"),
                    control: NodeControl {
                        enabled,
                        muted: false,
                    },
                    gain_db: None,
                }],
            )
            .unwrap();
        io.enabled = enabled;
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert_eq!(io.begin_calls, usize::try_from(revision - 1).unwrap());
        cycle.first_frame += 4;
    }
    assert_eq!((io.inner.reads, io.inner.writes), (1, 1));
}

#[test]
fn admission_retry_observes_off_without_consuming_pcm_dsp_or_timeline() {
    let counts = Arc::new(Counts::default());
    let mut processor = prepare_provider(&provider_document(NodeControl::default()), &counts);
    let bank = Arc::clone(processor.plan().controls());
    let exchange = crate::ProcessorExchange::new(1);
    let (_, inner, mut cycle) = callback_fixture();
    let mut io = ObservedControlIo {
        inner,
        enabled: true,
        begin_calls: 0,
        admission: CycleAdmission::Process,
    };
    assert!(
        processor
            .process_external_cycle(cycle, &mut io)
            .unwrap()
            .is_some()
    );
    assert_eq!(counts.calls.load(Ordering::Relaxed), 1);
    for (revision, enabled) in [(2, false), (3, true)] {
        let updates = ["in", "out"].map(|id| crate::RuntimeControlUpdate {
            node_id: NodeId::from(id),
            control: NodeControl {
                enabled,
                muted: false,
            },
            gain_db: None,
        });
        exchange
            .publish_controls(&bank, revision, &updates)
            .unwrap();
        // The repeated already-accepted coordinates are invalid. Pending controls must not be
        // sampled and the adapter must not be entered on a contract failure.
        assert!(processor.process_external_cycle(cycle, &mut io).is_err());
        assert_eq!(bank.applied_revision(), revision - 1);
        io.enabled = enabled;
        io.admission = CycleAdmission::Retry;
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        let retry = processor.process_external_cycle(
            ProcessCycle {
                first_frame: 4,
                ..cycle
            },
            &mut io,
        );
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        assert_eq!(retry.unwrap(), None);
        assert_eq!(bank.applied_revision(), revision);
        assert_eq!(counts.calls.load(Ordering::Relaxed), 1);
        assert_eq!((io.inner.reads, io.inner.writes), (1, 1));
    }
    assert_eq!(io.begin_calls, 3);
    io.admission = CycleAdmission::Process;
    cycle.first_frame = 4;
    assert!(
        processor
            .process_external_cycle(cycle, &mut io)
            .unwrap()
            .is_some()
    );
    assert_eq!(counts.calls.load(Ordering::Relaxed), 2);
    assert_eq!((io.inner.reads, io.inner.writes), (2, 2));
}

#[test]
fn device_bus_off_clears_previous_quantum_before_reenable() {
    let mut processor = prepare_bus(1, false, false);
    let plan = Arc::clone(processor.plan());
    let exchange = crate::ProcessorExchange::new(1);
    let (_, mut io, mut cycle) = callback_fixture();
    io.value = 0.75;
    processor.process_external_cycle(cycle, &mut io).unwrap();
    for (revision, enabled) in [(2, false), (3, true)] {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("bus"),
                    control: NodeControl {
                        enabled,
                        muted: false,
                    },
                    gain_db: None,
                }],
            )
            .unwrap();
        cycle.first_frame += 4;
        io.value = 0.25;
        processor.process_external_cycle(cycle, &mut io).unwrap();
        assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
    }
    cycle.first_frame += 4;
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert_eq!(io.output.map(f32::to_bits), [0.25_f32; 8].map(f32::to_bits));
}
