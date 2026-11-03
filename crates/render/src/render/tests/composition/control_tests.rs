use super::*;
use crate::{ProcessorExchange, RuntimeControlUpdate};
use mixrelay_audio_graph::{AudioDeviceOutputConfig, NodeControl};

fn control(composed: &ComposedGraph, id: &NodeId) -> NodeControl {
    composed
        .document
        .nodes
        .iter()
        .find(|node| &node.id == id)
        .expect("runtime control owner must be present")
        .control
}

#[test]
fn capture_owner_uses_any_enabled_reference_without_sharing_mute_or_gain() {
    let registry = mic_registry(vec![]);
    let document = GraphDocument::new(1, vec![], vec![]);
    let masks = vec![
        runtime_node_id("A", &NodeId::from("mic")),
        runtime_node_id("B", &NodeId::from("mic")),
    ];
    for (a_enabled, b_enabled) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut a = mic_reference(-6.0);
        a.control = NodeControl {
            enabled: a_enabled,
            muted: true,
        };
        let mut b = mic_reference(3.0);
        b.control.enabled = b_enabled;
        let graphs = [
            GraphFragment {
                id: "A",
                document: &document,
                source_references: std::slice::from_ref(&a),
                device_references: &[],
                output_references: &[],
            },
            GraphFragment {
                id: "B",
                document: &document,
                source_references: std::slice::from_ref(&b),
                device_references: &[],
                output_references: &[],
            },
        ];
        let composed = compose(&registry, &graphs, 1, context(), &GraphCompiler::default())
            .expect("valid shared capture references");
        let owner = &composed.native_sources["macbook-mic"];
        assert_eq!(composed.native_sources.len(), 1);
        assert_eq!(composed.native_control_bindings[owner], masks);
        assert_eq!(control(&composed, &masks[0]), a.control);
        assert_eq!(control(&composed, &masks[1]), b.control);
        assert_eq!(
            control(&composed, owner),
            NodeControl {
                enabled: a_enabled || b_enabled,
                muted: false,
            }
        );
        for (mask, expected_gain) in masks.iter().zip([-6.0_f32, 3.0]) {
            let node = composed
                .document
                .nodes
                .iter()
                .find(|node| &node.id == mask)
                .unwrap();
            let NodeConfig::Gain(config) = &node.config else {
                panic!("capture references must own their gain")
            };
            assert_eq!(config.gain_db.to_bits(), expected_gain.to_bits());
        }
    }
}

#[test]
fn virtual_owner_controls_cover_both_reference_directions_and_all_scaffolding() {
    let registry = DeviceRegistry::new(vec![device("X")]).unwrap();
    let document = GraphDocument::new(1, vec![], vec![]);
    for enabled in [false, true] {
        let mut local = reference("endpoint", "X", true, true);
        local.control = NodeControl {
            enabled,
            muted: true,
        };
        local.gain_db = -6.0;
        let composed = compose(
            &registry,
            &[GraphFragment {
                id: "A",
                document: &document,
                source_references: &[],
                device_references: &[local],
                output_references: &[],
            }],
            1,
            context(),
            &GraphCompiler::default(),
        )
        .unwrap();
        let read = reference_node_id("A", &NodeId::from("endpoint"), "read");
        let write = reference_node_id("A", &NodeId::from("endpoint"), "write");
        let masks = vec![read.clone(), write.clone()];
        assert_eq!(composed.native_control_bindings.len(), 3);
        for side in ["native", "mix", "snapshot"] {
            let owner = device_node_id(&VirtualEndpointId::from("X"), side);
            assert_eq!(composed.native_control_bindings[&owner], masks);
            assert_eq!(
                control(&composed, &owner),
                NodeControl {
                    enabled,
                    muted: false
                }
            );
        }
        for (id, port, expected_gain) in [
            (read, FROM_OS_PORT_ID, -6.0_f32),
            (write, TO_OS_PORT_ID, 0.0),
        ] {
            assert_eq!(
                control(&composed, &id),
                NodeControl {
                    enabled,
                    muted: true
                }
            );
            assert_eq!(
                composed.node_origins[&id].meter_port,
                Some(PortId::from(port))
            );
            let node = composed
                .document
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap();
            let NodeConfig::Gain(config) = &node.config else {
                panic!("virtual reference must be a local gain mask")
            };
            assert_eq!(config.gain_db.to_bits(), expected_gain.to_bits());
        }
    }
}

fn shared_physical_route(
    sources: &[SourceReference; 2],
    outputs: &[OutputReference; 2],
) -> ComposedGraph {
    let registry = mic_registry(vec![])
        .with_outputs(vec![SharedOutput {
            id: "headphones".into(),
            config: AudioDeviceOutputConfig {
                selector: DeviceSelector::DefaultOutput,
                input_channels: 2,
            },
        }])
        .unwrap();
    let document = GraphDocument::new(
        1,
        vec![],
        vec![edge("route", "mic", AUDIO_PORT_ID, "out", AUDIO_PORT_ID)],
    );
    let fragments = [
        GraphFragment {
            id: "A",
            document: &document,
            source_references: std::slice::from_ref(&sources[0]),
            output_references: std::slice::from_ref(&outputs[0]),
            device_references: &[],
        },
        GraphFragment {
            id: "B",
            document: &document,
            source_references: std::slice::from_ref(&sources[1]),
            output_references: std::slice::from_ref(&outputs[1]),
            device_references: &[],
        },
    ];
    compose(
        &registry,
        &fragments,
        1,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap()
}

fn headphone_reference(gain_db: f32, muted: bool) -> OutputReference {
    OutputReference {
        control: NodeControl {
            enabled: true,
            muted,
        },
        node_id: "out".into(),
        output_id: "headphones".into(),
        channels: 2,
        gain_db,
    }
}

#[test]
fn physical_output_owner_ignores_local_mute_and_keeps_silent_reference_bound() {
    let composed = shared_physical_route(
        &[mic_reference(0.0), mic_reference(0.0)],
        &[
            headphone_reference(-6.020_6, true),
            headphone_reference(-6.020_6, false),
        ],
    );
    let owner = &composed.native_outputs["headphones"];
    assert_eq!(control(&composed, owner), NodeControl::default());
    assert_eq!(
        composed.native_control_bindings[owner],
        vec![
            runtime_node_id("A", &"out".into()),
            runtime_node_id("B", &"out".into())
        ]
    );
    assert_frame(&pcm(&composed)[owner], [0.1, 0.2]);
}

fn source_update(
    id: &NodeId,
    enabled: bool,
    muted: bool,
    gain_db: Option<f32>,
) -> RuntimeControlUpdate {
    RuntimeControlUpdate {
        node_id: id.clone(),
        control: NodeControl { enabled, muted },
        gain_db,
    }
}

fn render_physical_frame(processor: &mut BlockProcessor) -> [f32; 8] {
    let source = [0.2, 0.4].repeat(4);
    let mut sink = [0.0; 8];
    processor
        .process_block(4, &[source.as_slice()], &mut [sink.as_mut_slice()])
        .unwrap();
    sink
}

#[test]
fn live_controls_keep_shared_pcm_owner_and_processor_without_topology_replacement() {
    let composed = shared_physical_route(
        &[mic_reference(0.0), mic_reference(-6.020_6)],
        &[
            headphone_reference(0.0, false),
            headphone_reference(0.0, false),
        ],
    );
    let plan = Arc::new(RenderPlan::prepare(&composed.execution).unwrap());
    let mut processor = BlockProcessor::new(plan.clone()).unwrap();
    let bank = plan.controls();
    let exchange = ProcessorExchange::new(1);
    let owner = &composed.native_sources["macbook-mic"];
    let masks = &composed.native_control_bindings[owner];
    assert_eq!(plan.source_bindings().len(), 1);
    assert_eq!(plan.sink_bindings().len(), 1);
    assert_frame(&render_physical_frame(&mut processor), [0.3, 0.6]);
    let steps = [
        (
            vec![source_update(&masks[0], false, false, None)],
            [0.1, 0.2],
        ),
        (vec![source_update(&masks[1], true, true, None)], [0.0, 0.0]),
        (
            vec![
                source_update(&masks[1], false, true, None),
                source_update(owner, false, false, None),
            ],
            [0.0, 0.0],
        ),
        (
            vec![
                source_update(&masks[0], true, false, Some(-6.020_6)),
                source_update(owner, true, false, None),
            ],
            [0.1, 0.2],
        ),
    ];
    for (index, (updates, expected)) in steps.into_iter().enumerate() {
        let revision = u64::try_from(index).unwrap() + 2;
        exchange.publish_controls(bank, revision, &updates).unwrap();
        assert_frame(&render_physical_frame(&mut processor), expected);
        assert_eq!(bank.applied_revision(), revision);
        assert_eq!(exchange.plan_revisions().active, 1);
        assert_eq!(exchange.plan_revisions().submitted, 1);
        assert!(Arc::ptr_eq(processor.plan(), &plan));
    }
}

#[test]
fn disabled_processor_retains_runtime_identity_and_routes() {
    let registry = DeviceRegistry::new(vec![]).unwrap();
    let mut off = gain("off", 0.0);
    off.control.enabled = false;
    let document = GraphDocument::new(
        1,
        vec![off, gain("sink", 0.0)],
        vec![edge("route", "off", AUDIO_PORT_ID, "sink", AUDIO_PORT_ID)],
    );
    let composed = compose(
        &registry,
        &[GraphFragment {
            id: "A",
            document: &document,
            source_references: &[],
            output_references: &[],
            device_references: &[],
        }],
        1,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    let id = runtime_node_id("A", &NodeId::from("off"));
    assert_eq!(composed.node_origins[&id].node_id, NodeId::from("off"));
    assert!(!control(&composed, &id).enabled);
    assert!(
        composed
            .execution
            .nodes()
            .iter()
            .any(|node| node.id() == &id)
    );
    assert_eq!(composed.document.edges.len(), 1);
    assert!(composed.native_control_bindings.is_empty());
}
