use super::*;

fn dynamics(kind: &str, bypass: bool) -> NodeConfig {
    serde_json::from_value(serde_json::json!({
        "type": kind,
        "config": if kind == "compressor" {
            serde_json::json!({"version":1,"channels":2,"bypass":bypass,"thresholdDb":-18,"ratio":4,"attackMs":0.1,"releaseMs":100,"makeupGainDb":0})
        } else {
            serde_json::json!({"version":1,"channels":2,"bypass":bypass,"ceilingDb":-6,"releaseMs":100})
        },
    }))
    .unwrap()
}

fn stateful_configs() -> Vec<NodeConfig> {
    let mut configs = vec![dynamics("compressor", false), dynamics("limiter", false)];
    configs.extend(
        builtin_effects::configs()
            .into_iter()
            .filter(|config| config.channels == 2)
            .map(NodeConfig::Effect),
    );
    configs
}

fn prepared(config: NodeConfig) -> BlockProcessor {
    let execution = compile(
        vec![device_input("in"), node("fx", config), device_output("out")],
        vec![
            edge("input", "in", AUDIO_PORT_ID, "fx", AUDIO_PORT_ID),
            edge("output", "fx", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap()
}

fn direct_block(processor: &mut BlockProcessor, inputs: &[&[f32]]) -> [f32; 8] {
    let mut output = [9.0; 8];
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    let result = processor.process_block(4, inputs, &mut [&mut output]);
    let operations = HEAP_OPERATIONS.with(|counter| counter.replace(None));
    result.unwrap();
    assert_eq!(operations, Some(0));
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

struct SampleIo {
    input: [f32; 8],
    output: [f32; 8],
}

impl ExternalIo for SampleIo {
    fn read_source(
        &mut self,
        _: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        destination.copy_from_slice(&self.input);
        TransferOutcome::complete(cycle.frames)
    }

    fn write_sink(
        &mut self,
        _: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.output.copy_from_slice(source);
        TransferOutcome::complete(cycle.frames)
    }
}

fn external_block(processor: &mut BlockProcessor, input: [f32; 8], first_frame: u64) -> [f32; 8] {
    external_block_report(processor, input, first_frame).0
}

fn external_block_report(
    processor: &mut BlockProcessor,
    input: [f32; 8],
    first_frame: u64,
) -> ([f32; 8], SampleFaults) {
    let mut io = SampleIo {
        input,
        output: [9.0; 8],
    };
    let cycle = ProcessCycle {
        clock: mixrelay_audio_io::process::ClockDomain {
            authority: mixrelay_audio_io::session::ClockAuthority::Device,
            id: 1,
            epoch: 1,
        },
        frames: 4,
        sample_rate: 48_000,
        first_frame,
        monotonic_ns: None,
        discontinuity: false,
    };
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    let result = processor.process_external_cycle(cycle, &mut io);
    let operations = HEAP_OPERATIONS.with(|counter| counter.replace(None));
    let report = result.unwrap().unwrap();
    assert_eq!(operations, Some(0));
    assert!(io.output.iter().all(|sample| sample.is_finite()));
    (io.output, report.sample_faults)
}

struct NormalizedSource {
    pcm: SampleIo,
    frames: u32,
    pending: u64,
}

impl ExternalIo for NormalizedSource {
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        let mut transfer = self.pcm.read_source(binding, destination, cycle);
        transfer.frames = self.frames;
        transfer.source_sample_faults = std::mem::take(&mut self.pending);
        if self.frames < cycle.frames {
            transfer.activity = mixrelay_audio_io::process::InputActivity::Priming;
        }
        transfer
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.pcm.write_sink(binding, source, cycle)
    }
}

fn normalized_cycle(
    processor: &mut BlockProcessor,
    input: &mut NormalizedSource,
    first_frame: u64,
) -> ProcessReport {
    let cycle = ProcessCycle {
        clock: mixrelay_audio_io::process::ClockDomain {
            authority: mixrelay_audio_io::session::ClockAuthority::Device,
            id: 1,
            epoch: 1,
        },
        frames: 4,
        sample_rate: 48_000,
        first_frame,
        monotonic_ns: None,
        discontinuity: false,
    };
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    let report = processor.process_external_cycle(cycle, input);
    let operations = HEAP_OPERATIONS.with(|counter| counter.replace(None));
    assert_eq!(operations, Some(0));
    report.unwrap().unwrap()
}

#[test]
fn upstream_and_graph_source_faults_combine_once_without_resetting_history() {
    for config in stateful_configs() {
        let mut processor = prepared(config.clone());
        let mut reference = prepared(config);
        let mut input = NormalizedSource {
            pcm: SampleIo {
                input: [0.25; 8],
                output: [0.0; 8],
            },
            frames: 4,
            pending: 0,
        };
        for block in 0..64_u64 {
            let mut clean = [0.25; 8];
            input.pcm.input = clean;
            if block == 8 {
                // Three native replacements already happened before SRC. A
                // fourth invalid value reaches graph admission independently.
                clean[..4].fill(0.0);
                input.pcm.input = clean;
                input.pcm.input[3] = f32::NAN;
                input.pending = 3;
            }
            let expected = external_block(&mut reference, clean, block * 4);
            let report = normalized_cycle(&mut processor, &mut input, block * 4);
            assert_eq!(
                input.pcm.output.map(f32::to_bits),
                expected.map(f32::to_bits)
            );
            assert_eq!(
                report.sample_faults.source_samples,
                if block == 8 { 4 } else { 0 }
            );
            assert_eq!(report.sample_faults.blocks, u64::from(block == 8));
            assert_eq!(report.discontinuities, 0);
        }
    }
}

#[test]
fn normalized_faults_report_once_on_empty_and_partial_source_reads_and_saturate() {
    for frames in [0, 2, 4] {
        let mut processor = prepared(dynamics("compressor", false));
        let mut input = NormalizedSource {
            pcm: SampleIo {
                input: [f32::NAN; 8],
                output: [0.0; 8],
            },
            frames,
            pending: u64::MAX,
        };
        let report = normalized_cycle(&mut processor, &mut input, 0);
        assert_eq!(report.sample_faults.source_samples, u64::MAX);
        assert_eq!(report.sample_faults.blocks, 1);
        assert_eq!(report.discontinuities, 0);
        assert!(input.pcm.output.iter().all(|sample| sample.is_finite()));
        input.pcm.input.fill(0.25);
        let report = normalized_cycle(&mut processor, &mut input, 4);
        assert_eq!(report.sample_faults, SampleFaults::default());
        assert_eq!(report.discontinuities, 0);
    }
}

#[test]
fn invalid_source_samples_match_zero_replacement_and_cannot_poison_effect_history() {
    let invalid = [
        f32::INFINITY,
        0.25,
        f32::NEG_INFINITY,
        -0.125,
        f32::NAN,
        0.25,
        0.25,
        -0.125,
    ];
    let replaced = [0.0, 0.25, 0.0, -0.125, 0.0, 0.25, 0.25, -0.125];
    let healthy = [0.25, -0.125, 0.25, -0.125, 0.25, -0.125, 0.25, -0.125];
    for config in stateful_configs() {
        let mut direct = prepared(config.clone());
        let mut external = prepared(config.clone());
        let mut reference = prepared(config);
        // Keep processing real, nonzero PCM after the corrupt block. An output-only
        // sanitizer would hide NaN while leaving a poisoned detector or delay line.
        // Sixteen thousand frames also cross the longest configured reverb
        // pre-delay/onset, so corrupt history cannot remain hidden in a buffer.
        for block in 0..4000_u64 {
            let input = if block == 0 { invalid } else { healthy };
            let clean = if block == 0 { replaced } else { healthy };
            let expected = direct_block(&mut reference, &[&clean]).map(f32::to_bits);
            assert_eq!(
                direct_block(&mut direct, &[&input]).map(f32::to_bits),
                expected
            );
            let (output, faults) = external_block_report(&mut external, input, block * 4);
            assert_eq!(output.map(f32::to_bits), expected);
            assert_eq!(
                faults,
                SampleFaults {
                    source_samples: if block == 0 { 3 } else { 0 },
                    blocks: u64::from(block == 0),
                    ..SampleFaults::default()
                },
            );
        }
    }
}

#[test]
fn common_boundary_preserves_finite_pcm_and_bypass_bits() {
    let input = [
        0.0,
        0.0,
        f32::from_bits(1),
        -f32::from_bits(1),
        1.25,
        -2.0,
        f32::MAX,
        -f32::MAX,
    ];
    for config in [
        NodeConfig::Gain(GainConfig {
            channels: 2,
            gain_db: 0.0,
        }),
        dynamics("compressor", true),
        dynamics("limiter", true),
    ] {
        let mut direct = prepared(config.clone());
        let mut external = prepared(config);
        assert_eq!(
            direct_block(&mut direct, &[&input]).map(f32::to_bits),
            input.map(f32::to_bits)
        );
        let (output, faults) = external_block_report(&mut external, input, 0);
        assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
        assert_eq!(faults, SampleFaults::default());
    }
}

#[test]
fn finite_gain_overflow_is_contained_before_downstream_dynamics() {
    let execution = compile(
        vec![
            device_input("in"),
            node(
                "gain",
                NodeConfig::Gain(GainConfig {
                    channels: 2,
                    gain_db: 6.020_600_3,
                }),
            ),
            node("fx", dynamics("compressor", false)),
            device_output("out"),
        ],
        vec![
            edge("gain-in", "in", AUDIO_PORT_ID, "gain", AUDIO_PORT_ID),
            edge("gain-fx", "gain", AUDIO_PORT_ID, "fx", AUDIO_PORT_ID),
            edge("fx-out", "fx", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let mut actual = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let mut reference = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let mut external = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let mut external_reference = BlockProcessor::new(plan).unwrap();
    let invalid = [
        f32::MAX,
        -f32::MAX,
        0.25,
        -0.125,
        0.25,
        -0.125,
        0.25,
        -0.125,
    ];
    let replaced = [0.0, 0.0, 0.25, -0.125, 0.25, -0.125, 0.25, -0.125];
    assert_eq!(
        direct_block(&mut actual, &[&invalid]).map(f32::to_bits),
        direct_block(&mut reference, &[&replaced]).map(f32::to_bits)
    );
    let (output, faults) = external_block_report(&mut external, invalid, 0);
    assert_eq!(
        output.map(f32::to_bits),
        external_block(&mut external_reference, replaced, 0).map(f32::to_bits),
    );
    assert_eq!(
        faults,
        SampleFaults {
            output_samples: 2,
            blocks: 1,
            ..SampleFaults::default()
        }
    );
    for block in 1..101 {
        let healthy = [0.25; 8];
        assert_eq!(
            direct_block(&mut actual, &[&healthy]).map(f32::to_bits),
            direct_block(&mut reference, &[&healthy]).map(f32::to_bits)
        );
        let (output, faults) = external_block_report(&mut external, healthy, block * 4);
        assert_eq!(
            output.map(f32::to_bits),
            external_block(&mut external_reference, healthy, block * 4).map(f32::to_bits),
        );
        assert_eq!(faults, SampleFaults::default());
    }
}

#[test]
fn finite_mixer_overflow_is_contained_before_downstream_dynamics() {
    let execution = compile(
        vec![
            device_input("a"),
            device_input("b"),
            node(
                "mix",
                NodeConfig::Mixer(MixerConfig {
                    channels: 2,
                    inputs: vec![PortId::from("a"), PortId::from("b")],
                }),
            ),
            node("fx", dynamics("limiter", false)),
            device_output("out"),
        ],
        vec![
            edge("a-mix", "a", AUDIO_PORT_ID, "mix", "a"),
            edge("b-mix", "b", AUDIO_PORT_ID, "mix", "b"),
            edge("mix-fx", "mix", AUDIO_PORT_ID, "fx", AUDIO_PORT_ID),
            edge("fx-out", "fx", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let mut actual = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let mut reference = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let mut external = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let mut external_reference = BlockProcessor::new(plan).unwrap();
    let invalid = [
        f32::MAX,
        -f32::MAX,
        0.125,
        -0.0625,
        0.125,
        -0.0625,
        0.125,
        -0.0625,
    ];
    let replaced = [0.0, 0.0, 0.125, -0.0625, 0.125, -0.0625, 0.125, -0.0625];
    assert_eq!(
        direct_block(&mut actual, &[&invalid, &invalid]).map(f32::to_bits),
        direct_block(&mut reference, &[&replaced, &replaced]).map(f32::to_bits)
    );
    let (output, faults) = external_block_report(&mut external, invalid, 0);
    assert_eq!(
        output.map(f32::to_bits),
        external_block(&mut external_reference, replaced, 0).map(f32::to_bits),
    );
    assert_eq!(
        faults,
        SampleFaults {
            output_samples: 2,
            blocks: 1,
            ..SampleFaults::default()
        }
    );
    for block in 1..101 {
        let healthy = [0.125; 8];
        assert_eq!(
            direct_block(&mut actual, &[&healthy, &healthy]).map(f32::to_bits),
            direct_block(&mut reference, &[&healthy, &healthy]).map(f32::to_bits)
        );
        let (output, faults) = external_block_report(&mut external, healthy, block * 4);
        assert_eq!(
            output.map(f32::to_bits),
            external_block(&mut external_reference, healthy, block * 4).map(f32::to_bits),
        );
        assert_eq!(faults, SampleFaults::default());
    }
}

#[test]
fn route_fan_in_overflow_is_contained_before_effects_and_sinks() {
    let input = [
        f32::MAX,
        -f32::MAX,
        0.125,
        -0.0625,
        0.125,
        -0.0625,
        0.125,
        -0.0625,
    ];
    let replaced = [0.0, 0.0, 0.125, -0.0625, 0.125, -0.0625, 0.125, -0.0625];
    for with_effect in [false, true] {
        let mut nodes = vec![device_input("a"), device_input("b"), device_output("out")];
        let destination = if with_effect { "fx" } else { "out" };
        let mut edges = vec![
            edge("a-target", "a", AUDIO_PORT_ID, destination, AUDIO_PORT_ID),
            edge("b-target", "b", AUDIO_PORT_ID, destination, AUDIO_PORT_ID),
        ];
        if with_effect {
            nodes.push(node("fx", dynamics("compressor", false)));
            edges.push(edge("fx-out", "fx", AUDIO_PORT_ID, "out", AUDIO_PORT_ID));
        }
        let execution = compile(nodes, edges);
        let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
        let mut direct = BlockProcessor::new(Arc::clone(&plan)).unwrap();
        let mut reference = BlockProcessor::new(Arc::clone(&plan)).unwrap();
        let mut external = BlockProcessor::new(Arc::clone(&plan)).unwrap();
        let mut external_reference = BlockProcessor::new(plan).unwrap();
        for block in 0..101 {
            let actual = if block == 0 { input } else { [0.125; 8] };
            let clean = if block == 0 { replaced } else { [0.125; 8] };
            assert_eq!(
                direct_block(&mut direct, &[&actual, &actual]).map(f32::to_bits),
                direct_block(&mut reference, &[&clean, &clean]).map(f32::to_bits),
            );
            let (output, faults) = external_block_report(&mut external, actual, block * 4);
            assert_eq!(
                output.map(f32::to_bits),
                external_block(&mut external_reference, clean, block * 4).map(f32::to_bits),
            );
            assert_eq!(
                faults,
                SampleFaults {
                    input_samples: if block == 0 { 2 } else { 0 },
                    blocks: u64::from(block == 0),
                    ..SampleFaults::default()
                },
            );
        }
    }
}

struct CorruptOutputProcessor {
    first_block: bool,
    corrupt: bool,
}

impl HostedAudioProcessor for CorruptOutputProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], _: usize) -> bool {
        output.copy_from_slice(input);
        if self.first_block {
            output[..3].copy_from_slice(if self.corrupt {
                &[f32::INFINITY, f32::NAN, f32::NEG_INFINITY]
            } else {
                &[0.0; 3]
            });
            self.first_block = false;
        }
        true
    }
}

struct CorruptOutputFactory(bool);

impl HostedProcessorFactory for CorruptOutputFactory {
    fn prepare(
        &self,
        _: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        Ok(Box::new(CorruptOutputProcessor {
            first_block: true,
            corrupt: self.0,
        }))
    }
}

#[test]
fn successful_hosted_nonfinite_output_cannot_poison_downstream_history() {
    let document = GraphDocument::new(
        1,
        vec![
            device_input("in"),
            node(
                "provider",
                NodeConfig::Provider(ProviderNodeConfig {
                    ports: None,
                    type_id: "test:corrupt".into(),
                    parameters: serde_json::json!({}),
                }),
            ),
            node("fx", dynamics("compressor", false)),
            device_output("out"),
        ],
        vec![
            edge("in-host", "in", AUDIO_PORT_ID, "provider", AUDIO_PORT_ID),
            edge("host-fx", "provider", AUDIO_PORT_ID, "fx", AUDIO_PORT_ID),
            edge("fx-out", "fx", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let execution = GraphCompiler::new(TestProviderResolver)
        .compile(
            &document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    let make = |corrupt| {
        BlockProcessor::prepare_with_hosted_processors(
            &execution,
            Arc::new(crate::PeakMeterBank::new(8, 2)),
            &CorruptOutputFactory(corrupt),
        )
        .unwrap()
    };
    let mut actual = make(true);
    let mut reference = make(false);
    for _ in 0..1000 {
        let healthy = [0.25; 8];
        assert_eq!(
            direct_block(&mut actual, &[&healthy]).map(f32::to_bits),
            direct_block(&mut reference, &[&healthy]).map(f32::to_bits)
        );
    }
}
