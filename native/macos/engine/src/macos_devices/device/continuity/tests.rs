use crate::macos_devices::{
    DeviceSet, bridge::NEXT_DEVICE_EPOCH, ffi, tests::synthetic_device, worker::WorkerContext,
};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping,
    CompileContext, CompileLimits, DelayParameters, DeviceSelector, EdgeSpec, Effect, EffectConfig,
    GraphCompiler, GraphDocument, MixerConfig, NodeConfig, NodeControl, NodeSpec, RenderFormat,
};
use mixrelay_audio_io::process::{
    ClockDomain, InputActivity, ProcessCycle, ProcessReport, TransferOutcome,
};
use mixrelay_audio_io::session::ClockAuthority;
use mixrelay_render::{BlockProcessor, ExternalBinding, ExternalIo, RenderPlan};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};

#[test]
fn startup_loss_and_recovery_markers_are_distinct_and_faults_survive_empty_reads() {
    let (device, context) = synthetic_device(1, 48_000, 48_000, true);
    context.ready_rate.store(0, Ordering::Release);
    let mut pcm = [9.0; 32];
    for _ in 0..2 {
        let initial = device.read(&mut pcm, 0);
        assert_eq!(initial.activity, InputActivity::Priming);
        assert!(!initial.discontinuity);
    }
    context.ready_rate.store(48_000, Ordering::Release);
    assert!(!device.read(&mut pcm, 0).discontinuity); // Initial native epoch.
    context.ready_rate.store(0, Ordering::Release);
    context.native.source_sample_faults.record(3);
    let lost = device.read(&mut pcm, 0);
    assert_eq!(lost.frames, 0);
    assert!(lost.discontinuity);
    assert_eq!(lost.source_sample_faults, 3);
    let repeated = device.read(&mut pcm, 0);
    assert!(!repeated.discontinuity);
    assert_eq!(repeated.source_sample_faults, 0);
    context.epoch.store(
        NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
        Ordering::Release,
    );
    context.ready_rate.store(48_000, Ordering::Release);
    assert!(!device.read(&mut pcm, 0).discontinuity); // Already reset at loss.
    context.epoch.store(
        NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
        Ordering::Release,
    );
    assert!(device.read(&mut pcm, 0).discontinuity); // A separate unseen loss.
    assert!(!device.read(&mut pcm, 0).discontinuity);
}

struct Io {
    devices: DeviceSet,
    output: [[f32; 32]; 3],
    epochs: [Cell<Option<u64>>; 2],
    replace_during_read: Cell<Option<u64>>,
}

impl ExternalIo for Io {
    fn source_epoch(&self, binding: &ExternalBinding) -> Option<u64> {
        self.epochs[usize::from(binding.node_id().as_str() != "source-a")].get()
    }

    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        _: &ProcessCycle,
    ) -> TransferOutcome {
        let transfer = self
            .devices
            .read(binding.node_id().as_str(), destination, 0)
            .unwrap();
        if binding.node_id().as_str() == "source-a"
            && let Some(epoch) = self.replace_during_read.take()
        {
            self.epochs[0].set(Some(epoch));
        }
        transfer
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        let index = match binding.node_id().as_str() {
            "sink-a" => 0,
            "sink-b" => 1,
            "sink-mixed" => 2,
            _ => panic!("unexpected sink"),
        };
        self.output[index].copy_from_slice(source);
        TransferOutcome::complete(cycle.frames)
    }
}

struct Fixture {
    processor: BlockProcessor,
    io: Io,
    contexts: [WorkerContext; 2],
    frame: u64,
}

impl Fixture {
    fn new() -> Self {
        let (a, context_a) = synthetic_device(1, 48_000, 48_000, true);
        let (b, context_b) = synthetic_device(1, 48_000, 48_000, true);
        Self {
            processor: processor(),
            io: Io {
                devices: DeviceSet(
                    BTreeMap::from([("source-a".into(), a), ("source-b".into(), b)]),
                    BTreeMap::new(),
                ),
                output: [[0.0; 32]; 3],
                epochs: [Cell::new(None), Cell::new(None)],
                replace_during_read: Cell::new(None),
            },
            contexts: [context_a, context_b],
            frame: 0,
        }
    }

    fn feed(&self, index: usize, value: f32) {
        // SAFETY: unpublished fixture serializes producer and graph roles; PCM
        // has the complete mono extent and is disjoint from ring storage.
        assert_eq!(
            unsafe {
                ffi::mr_device_io_bridge_write(
                    self.contexts[index].native.pointer.as_ptr(),
                    [value; 32].as_ptr(),
                    32,
                )
            },
            32
        );
    }

    fn run(&mut self) -> ProcessReport {
        let cycle = ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::Device,
                id: 1,
                epoch: 0,
            },
            sample_rate: 48_000,
            frames: 32,
            first_frame: self.frame,
            monotonic_ns: None,
            discontinuity: false,
        };
        let report = self
            .processor
            .process_external_cycle(cycle, &mut self.io)
            .unwrap()
            .unwrap();
        self.frame += 32;
        report
    }
}

#[test]
fn first_empty_loss_resets_only_affected_dsp_and_recovery_does_not_reset_mixed_history_twice() {
    let mut fixture = Fixture::new();
    fixture.run(); // Admit both graph consumer epochs before supplying PCM.
    for _ in 0..12 {
        fixture.feed(0, 0.5);
        fixture.feed(1, 0.5);
        fixture.run();
    }
    assert!(
        fixture
            .io
            .output
            .iter()
            .all(|block| block.iter().any(|sample| *sample > 0.4))
    );
    fixture.feed(0, 0.5); // Must not be replayed when recovery admits its epoch.
    fixture.contexts[0].ready_rate.store(0, Ordering::Release);
    fixture.contexts[0].native.source_sample_faults.record(3);
    let allocated = allocation_counter::measure(|| {
        fixture.feed(1, 0.25);
        let loss = fixture.run();
        assert_eq!(loss.discontinuities, 1);
        assert_eq!(loss.sample_faults.source_samples, 3);
        assert!(
            fixture.io.output[0]
                .iter()
                .all(|sample| sample.to_bits() == 0)
        );
        assert!(
            fixture.io.output[2]
                .iter()
                .all(|sample| sample.to_bits() == 0)
        );
        assert!(fixture.io.output[1].iter().any(|sample| *sample > 0.4));
        for _ in 0..4 {
            fixture.feed(1, 0.25);
            let repeated = fixture.run();
            assert_eq!(repeated.discontinuities, 0);
            assert_eq!(repeated.sample_faults.source_samples, 0);
        }
        assert!(fixture.io.output[2].iter().all(|sample| *sample > 0.2));
        fixture.contexts[0].epoch.store(
            NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed),
            Ordering::Release,
        );
        fixture.contexts[0]
            .ready_rate
            .store(48_000, Ordering::Release);
        fixture.feed(1, 0.25);
        assert_eq!(fixture.run().discontinuities, 0);
        assert!(
            fixture.io.output[0]
                .iter()
                .all(|sample| sample.to_bits() == 0)
        );
        assert!(fixture.io.output[2].iter().all(|sample| *sample > 0.2));
        for _ in 0..2 {
            fixture.feed(0, 0.75);
            fixture.feed(1, 0.25);
            assert_eq!(fixture.run().discontinuities, 0);
        }
        assert!(
            fixture.io.output[0]
                .iter()
                .any(|sample| (*sample - 0.75).abs() < 1e-6)
        );
        assert!(
            fixture.io.output[0]
                .iter()
                .all(|sample| *sample == 0.0 || (*sample - 0.75).abs() < 1e-6)
        );
    });
    assert_eq!(allocated.count_total, 0);
}

#[test]
fn tracked_source_loss_and_racing_session_replacement_reset_only_their_downstream_tails() {
    let mut fixture = Fixture::new();
    fixture.io.epochs[0].set(Some(7));
    fixture.run();
    for _ in 0..12 {
        fixture.feed(0, 0.5);
        fixture.feed(1, 0.5);
        fixture.run();
    }
    let allocated = allocation_counter::measure(|| {
        fixture.io.epochs[0].set(Some(0));
        fixture.feed(0, 0.875); // A retired but pinned producer may still return PCM.
        fixture.feed(1, 0.25);
        assert_eq!(fixture.run().discontinuities, 1);
        assert!(
            fixture.io.output[0]
                .iter()
                .all(|sample| sample.to_bits() == 0)
        );
        assert!(
            fixture.io.output[2]
                .iter()
                .all(|sample| sample.to_bits() == 0)
        );
        assert!(fixture.io.output[1].iter().any(|sample| *sample > 0.4));
        for _ in 0..4 {
            fixture.feed(1, 0.25);
            assert_eq!(fixture.run().discontinuities, 0);
        }
        fixture.io.epochs[0].set(Some(8));
        fixture.feed(0, 0.75);
        fixture.feed(1, 0.25);
        assert_eq!(fixture.run().discontinuities, 0);
        assert!(fixture.io.output[2].iter().all(|sample| *sample > 0.2));
        for _ in 0..3 {
            fixture.feed(0, 0.75);
            fixture.feed(1, 0.25);
            fixture.run();
        }
        fixture.feed(0, 0.875);
        fixture.feed(1, 0.25);
        fixture.io.replace_during_read.set(Some(9));
        assert_eq!(fixture.run().discontinuities, 1);
        assert!(
            fixture.io.output[0]
                .iter()
                .all(|sample| sample.to_bits() == 0)
        );
        for _ in 0..3 {
            fixture.feed(0, 0.125);
            fixture.feed(1, 0.25);
            assert_eq!(fixture.run().discontinuities, 0);
        }
        assert!(fixture.io.output[0].iter().all(|sample| *sample < 0.3));
    });
    assert_eq!(allocated.count_total, 0);
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        label: None,
        control: NodeControl::default(),
        config,
    }
}

fn edge(id: &str, source: &str, destination: &str, port: &str) -> EdgeSpec {
    EdgeSpec {
        id: id.into(),
        source: AudioEndpoint {
            node_id: source.into(),
            port_id: AUDIO_PORT_ID.into(),
        },
        destination: AudioEndpoint {
            node_id: destination.into(),
            port_id: port.into(),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}

fn processor() -> BlockProcessor {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for branch in ["a", "b", "mixed"] {
        let source = format!("source-{branch}");
        let delay = format!("delay-{branch}");
        let sink = format!("sink-{branch}");
        nodes.push(node(
            &source,
            if branch == "mixed" {
                NodeConfig::Mixer(MixerConfig {
                    inputs: vec!["a".into(), "b".into()],
                    channels: 1,
                })
            } else {
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::DefaultInput,
                    output_channels: 1,
                    gain_db: 0.0,
                })
            },
        ));
        nodes.push(node(
            &delay,
            NodeConfig::Effect(EffectConfig {
                version: 1,
                channels: 1,
                bypass: false,
                effect: Effect::Delay(DelayParameters {
                    time_ms: 1.0,
                    feedback: 0.5,
                    mix: 1.0,
                    mode: mixrelay_audio_graph::DelayMode::Independent,
                }),
            }),
        ));
        nodes.push(node(
            &sink,
            NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: DeviceSelector::DefaultOutput,
                input_channels: 1,
            }),
        ));
        edges.push(edge(
            &format!("in-{branch}"),
            &source,
            &delay,
            AUDIO_PORT_ID,
        ));
        edges.push(edge(&format!("out-{branch}"), &delay, &sink, AUDIO_PORT_ID));
    }
    edges.push(edge("mix-a", "source-a", "source-mixed", "a"));
    edges.push(edge("mix-b", "source-b", "source-mixed", "b"));
    let execution = GraphCompiler::default()
        .compile(
            &GraphDocument::new(1, nodes, edges),
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap()
}
