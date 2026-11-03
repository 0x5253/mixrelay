use super::*;
use mixrelay_audio_graph::{AudioDeviceOutputConfig, ChannelMapping, ChannelRoute};

fn output(id: &str) -> SharedOutput {
    SharedOutput {
        id: id.into(),
        config: AudioDeviceOutputConfig {
            selector: DeviceSelector::PersistentUid { uid: id.into() },
            input_channels: 2,
        },
    }
}

fn sink_reference(id: &str, gain_db: f32) -> OutputReference {
    OutputReference {
        control: mixrelay_audio_graph::NodeControl::default(),
        node_id: NodeId::from("out"),
        output_id: id.into(),
        channels: 2,
        gain_db,
    }
}

fn route() -> GraphDocument {
    GraphDocument::new(
        1,
        vec![],
        vec![edge("route", "mic", AUDIO_PORT_ID, "out", AUDIO_PORT_ID)],
    )
}

fn fragment<'a>(
    id: &'a str,
    document: &'a GraphDocument,
    sources: &'a [SourceReference],
    outputs: &'a [OutputReference],
) -> GraphFragment<'a> {
    GraphFragment {
        id,
        document,
        source_references: sources,
        output_references: outputs,
        device_references: &[],
    }
}

#[test]
fn physical_output_sums_graphs_once_and_removing_one_reference_keeps_other_audio() {
    let registry = mic_registry(vec![])
        .with_outputs(vec![output("headphones")])
        .unwrap();
    let document = route();
    let sources = [mic_reference(0.0)];
    let a_outputs = [sink_reference("headphones", 0.0)];
    let b_outputs = [sink_reference("headphones", -6.020_6)];
    let a = fragment("A", &document, &sources, &a_outputs);
    let b = fragment("B", &document, &sources, &b_outputs);
    let composed = compose(&registry, &[a, b], 2, context(), &GraphCompiler::default()).unwrap();
    assert_eq!(composed.native_outputs.len(), 1);
    assert_eq!(
        composed
            .document
            .nodes
            .iter()
            .filter(|node| matches!(node.config, NodeConfig::AudioDeviceOutput(_)))
            .count(),
        1
    );
    assert_frame(
        &pcm(&composed)[&composed.native_outputs["headphones"]],
        [0.3, 0.6],
    );
    let only_b = compose(
        &registry,
        &[fragment("B", &document, &sources, &b_outputs)],
        3,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    assert_eq!(only_b.native_outputs, composed.native_outputs);
    assert_frame(
        &pcm(&only_b)[&only_b.native_outputs["headphones"]],
        [0.1, 0.2],
    );
    assert_eq!(
        only_b.node_origins[&runtime_node_id("B", &NodeId::from("out"))].node_id,
        NodeId::from("out")
    );
}

#[test]
fn physical_output_mapping_keeps_other_output_device_independent() {
    let registry = mic_registry(vec![])
        .with_outputs(vec![output("headphones"), output("speakers")])
        .unwrap();
    let mut mapped = route();
    mapped.edges[0].channel_mapping = ChannelMapping::Explicit {
        routes: vec![ChannelRoute {
            source_channel: 1,
            destination_channel: 0,
        }],
    };
    let plain = route();
    let mut mono = sink_reference("headphones", 0.0);
    mono.channels = 1;
    let composed = compose(
        &registry,
        &[
            fragment("A", &mapped, &[mic_reference(0.0)], &[mono]),
            fragment(
                "B",
                &plain,
                &[mic_reference(0.0)],
                &[sink_reference("speakers", -6.020_6)],
            ),
        ],
        2,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    let samples = pcm(&composed);
    assert_eq!(composed.native_outputs.len(), 2);
    assert_frame(&samples[&composed.native_outputs["headphones"]], [0.4, 0.0]);
    assert_frame(&samples[&composed.native_outputs["speakers"]], [0.1, 0.2]);
}

#[test]
fn physical_output_alias_lowering_requires_resolver_and_preserves_routes() {
    let registry = mic_registry(vec![])
        .with_outputs(vec![output("headphones")])
        .unwrap();
    let mut document = route();
    document.nodes.push(NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from("mic"),
        label: None,
        config: mic_config(),
    });
    document.nodes.push(NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from("out"),
        label: Some("Default output".into()),
        config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: 2,
        }),
    });
    let mut ids = ResolvedNodeIdentities {
        sources: BTreeMap::from([(NodeId::from("mic"), "macbook-mic".into())]),
        outputs: BTreeMap::new(),
    };
    assert!(lower_document(&document, &registry, &ids).is_err());
    ids.outputs.insert(NodeId::from("out"), "headphones".into());
    let lowered = lower_document(&document, &registry, &ids).unwrap();
    assert_eq!(lowered.document.edges, document.edges);
    document.nodes[1].config = NodeConfig::AudioDeviceOutput(output("headphones").config);
    let resolved = lower_document(&document, &registry, &ids).unwrap();
    let composed = compose(
        &registry,
        &[lowered.as_fragment("A"), resolved.as_fragment("B")],
        2,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    assert_eq!(composed.native_outputs.len(), 1);
    assert_frame(
        &pcm(&composed)[&composed.native_outputs["headphones"]],
        [0.4, 0.8],
    );
}

#[test]
fn physical_output_rejects_unknown_width_reversed_edges_and_duplicate_identity() {
    let registry = mic_registry(vec![])
        .with_outputs(vec![output("headphones")])
        .unwrap();
    let mut wide = sink_reference("headphones", 0.0);
    wide.channels = 3;
    for reference in [wide, sink_reference("missing", 0.0)] {
        assert!(
            compose(
                &registry,
                &[fragment("A", &route(), &[mic_reference(0.0)], &[reference])],
                2,
                context(),
                &GraphCompiler::default()
            )
            .is_err()
        );
    }
    let reversed = GraphDocument::new(
        1,
        vec![],
        vec![edge("invalid", "out", AUDIO_PORT_ID, "out", AUDIO_PORT_ID)],
    );
    assert!(
        compose(
            &registry,
            &[fragment(
                "A",
                &reversed,
                &[],
                &[sink_reference("headphones", 0.0)]
            )],
            2,
            context(),
            &GraphCompiler::default()
        )
        .is_err()
    );
    assert!(
        DeviceRegistry::new(vec![])
            .unwrap()
            .with_outputs(vec![output("same"), output("same")])
            .is_err()
    );
}
