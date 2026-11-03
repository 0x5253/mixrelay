use super::*;
use mixrelay_audio_engine::ProcessorExchange;
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, GainConfig, GraphCompiler, GraphDocument, NodeConfig, NodeSpec,
    RenderFormat,
};
use mixrelay_engine::{RuntimeStatusBackend, session::AdapterRuntimeStatus};
use std::sync::Arc;

struct ControlFixture {
    status: Arc<AdapterRuntimeStatus>,
    devices: crate::macos_devices::MacDevices,
    exchange: ProcessorExchange,
}

impl ControlFixture {
    fn new() -> Self {
        let status = Arc::new(AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        Self {
            devices: crate::macos_devices::MacDevices::without_devices(
                Arc::clone(&status),
                48_000,
                32,
            ),
            status,
            exchange: ProcessorExchange::new(1),
        }
    }

    fn context(&self) -> AudioContext<'_> {
        AudioContext {
            exchange: &self.exchange,
            runtime_status: &self.status,
            devices: &self.devices,
        }
    }
}

fn document_with_internal_node(revision: u64, added: bool) -> GraphDocument {
    let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2, "revision": revision, "nodes": [
            {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
            {"id":"output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}}
        ], "edges": [
            {"id":"wire","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}}
        ]
    })).unwrap();
    if added {
        document.nodes.push(NodeSpec {
            id: "other-gain".into(),
            label: None,
            control: mixrelay_audio_graph::NodeControl::default(),
            config: NodeConfig::Gain(GainConfig {
                channels: 2,
                gain_db: -6.0,
            }),
        });
    }
    document
}

fn prepare(document: &GraphDocument, sample_rate: u32) -> Box<BlockProcessor> {
    let execution = GraphCompiler::default()
        .compile(
            document,
            CompileContext {
                format: RenderFormat {
                    sample_rate,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

fn processor_with_internal_node(revision: u64, added: bool) -> Box<BlockProcessor> {
    prepare(&document_with_internal_node(revision, added), 48_000)
}

fn render_passthrough(renderer: &mut Renderer<'_>) {
    let mut input = [0.25; 64];
    let mut output = [0.0; 64];
    let source = tests::stereo_list(&mut input);
    let mut sink = tests::stereo_list(&mut output);
    // SAFETY: separate live stereo buffers and their ABL metadata cover exactly
    // 32 frames. The unpublished fixture invokes this renderer synchronously.
    assert!(unsafe {
        renderer.render((&raw const source).cast(), (&raw mut sink).cast(), 32, None)
    });
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
}

#[test]
fn internal_add_remove_keeps_callback_pcm_and_frame_epoch_continuous() {
    let control = ControlFixture::new();
    let mut processor = processor_with_internal_node(1, false);
    let installed = Arc::clone(processor.plan());
    let signals = RenderSignals::default();
    let bindings = tests::stereo_bindings();
    let mut renderer = Renderer {
        timebase: mixrelay_realtime::Timebase::current(),
        processor: &mut processor,
        activation: Some(GraphActivation {
            installed: &installed,
            context: control.context(),
            rebind: &signals.rebind,
        }),
        bindings: &bindings,
        clock_id: 7,
        epoch: 19,
        native_clock: None,
        status: &control.status,
        position: None,
        progress: &signals.progress,
        failed: &signals.failed,
        invalid: &signals.invalid,
    };
    let address = std::ptr::from_ref(&renderer);
    render_passthrough(&mut renderer);
    for revision in 2..=101 {
        control
            .exchange
            .submit(processor_with_internal_node(revision, revision % 2 == 0))
            .unwrap();
        render_passthrough(&mut renderer);
        assert_eq!(renderer.processor.plan().revision(), revision);
        assert_eq!(std::ptr::from_ref(&renderer), address);
        assert!(!signals.rebind.load(Ordering::Acquire));
    }
    assert_eq!((renderer.position, renderer.epoch), (Some(101 * 32), 19));
    assert_eq!(signals.progress.load(Ordering::Acquire), 101);
    let status = control.status.status(installed.format());
    assert_eq!((status.underruns, status.overruns), (0, 0));
}

#[test]
fn latency_only_publication_passes_the_direct_callback_guard_without_rebind() {
    let control = ControlFixture::new();
    let mut processor = processor_with_internal_node(1, false);
    let installed = Arc::clone(processor.plan());
    let rebind = AtomicBool::new(false);
    let activation = GraphActivation {
        installed: &installed,
        context: control.context(),
        rebind: &rebind,
    };
    control.exchange.observe_plan(Arc::clone(&installed));
    assert!(control.exchange.refresh_latency().unwrap());
    assert!(!control.exchange.has_pending());
    assert!(control.exchange.has_pending_latency());
    let before = control.exchange.revisions();
    assert!(activation.activate(&mut processor));
    assert!(!control.exchange.has_pending_latency());
    assert!(!rebind.load(Ordering::Acquire));
    assert_eq!(control.exchange.revisions(), before);
    assert!(Arc::ptr_eq(processor.plan(), &installed));
}

#[test]
fn incompatible_format_is_left_pending_for_native_rebind() {
    let control = ControlFixture::new();
    let mut processor = tests::processor_at(32, 48_000);
    let installed = Arc::clone(processor.plan());
    let rebind = AtomicBool::new(false);
    let activation = GraphActivation {
        installed: &installed,
        context: control.context(),
        rebind: &rebind,
    };
    control
        .exchange
        .submit(prepare(&document_with_internal_node(2, false), 96_000))
        .unwrap();
    assert!(activation.activate(&mut processor));
    assert!(rebind.load(Ordering::Acquire));
    assert!(control.exchange.has_pending());
    assert!(Arc::ptr_eq(processor.plan(), &installed));
}

#[test]
fn changed_external_identity_or_channels_require_new_native_mapping() {
    let original = processor_with_internal_node(1, false);
    for index in 0..2 {
        let mut document = document_with_internal_node(2, false);
        document.edges.clear();
        document.nodes[index].id = "replacement".into();
        let changed = prepare(&document, 48_000);
        assert!(!same_external_geometry(original.plan(), changed.plan()));

        document.nodes[index].id = if index == 0 { "input" } else { "output" }.into();
        match &mut document.nodes[index].config {
            NodeConfig::AudioDeviceInput(config) => config.output_channels = 1,
            NodeConfig::AudioDeviceOutput(config) => config.input_channels = 1,
            _ => unreachable!("fixture contains only hardware endpoints"),
        }
        let changed = prepare(&document, 48_000);
        assert!(!same_external_geometry(original.plan(), changed.plan()));
    }
}
