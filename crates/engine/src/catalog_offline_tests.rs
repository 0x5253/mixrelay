use super::*;

fn offline_graph(uid: &str, channels: u16) -> GraphDocument {
    GraphDocument::new(
        1,
        vec![
            node(
                "input",
                NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::PersistentUid { uid: uid.into() },
                    output_channels: channels,
                    gain_db: -6.0206,
                }),
            ),
            node(
                "output",
                NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::PersistentUid { uid: uid.into() },
                    input_channels: channels,
                }),
            ),
        ],
        vec![route(
            "link",
            "input",
            AUDIO_PORT_ID,
            "output",
            AUDIO_PORT_ID,
        )],
    )
}

#[test]
fn absent_uid_retains_authored_documents_and_one_widest_native_owner() {
    let apps = Apps(Cell::new(0));
    let mut documents = vec![
        ("A".into(), offline_graph("unplugged", 1)),
        ("B".into(), offline_graph("unplugged", 4)),
    ];
    let before = documents.clone();
    let first = compile(&documents, &[], &apps).unwrap();
    assert_eq!(documents, before);
    assert_eq!(first.native_sources.len(), 1);
    assert_eq!(first.native_outputs.len(), 1);
    let plan = RenderPlan::prepare(&first.execution).unwrap();
    assert_eq!(plan.source_bindings()[0].channels(), 4);
    assert_eq!(plan.sink_bindings()[0].channels(), 4);
    documents.reverse();
    let reversed = compile(&documents, &[], &apps).unwrap();
    assert_eq!(first.native_sources, reversed.native_sources);
    assert_eq!(first.native_outputs, reversed.native_outputs);
    assert_eq!(
        RenderPlan::prepare(&reversed.execution)
            .unwrap()
            .source_bindings()[0]
            .channels(),
        4
    );
    let remaining = compile(&documents[1..], &[], &apps).unwrap();
    assert_eq!(remaining.native_sources, first.native_sources);
    assert_eq!(
        RenderPlan::prepare(&remaining.execution)
            .unwrap()
            .source_bindings()[0]
            .channels(),
        1
    );
}

#[test]
fn missing_defaults_never_alias_a_literal_uid_or_invent_a_native_device() {
    let apps = Apps(Cell::new(0));
    let documents = vec![
        (
            "defaults".into(),
            physical_graph(DeviceSelector::DefaultInput, 0.0),
        ),
        (
            "explicit".into(),
            offline_graph("unresolved-default:input", 2),
        ),
    ];
    let composed = compile(&documents, &[], &apps).unwrap();
    assert_eq!(composed.native_sources.len(), 2);
    assert_eq!(composed.native_outputs.len(), 2);
    assert!(composed.document.nodes.iter().any(|node| matches!(
        &node.config,
        NodeConfig::AudioDeviceInput(config) if config.selector == DeviceSelector::DefaultInput
    )));
    assert!(composed.document.nodes.iter().any(|node| matches!(
        &node.config,
        NodeConfig::AudioDeviceOutput(config) if config.selector == DeviceSelector::DefaultOutput
    )));
    assert_eq!(apps.0.get(), 0);
    for selector in [
        DeviceSelector::DefaultOutput,
        DeviceSelector::PersistentUid { uid: String::new() },
    ] {
        assert!(compile(&[("bad".into(), physical_graph(selector, 0.0))], &[], &apps).is_err());
    }
    for channels in [0, 65] {
        assert!(
            compile(
                &[("bad".into(), offline_graph("absent", channels))],
                &[],
                &apps
            )
            .is_err()
        );
    }
}

#[test]
fn silent_offline_bindings_do_not_interrupt_healthy_native_pcm() {
    let apps = Apps(Cell::new(0));
    let documents = vec![
        ("offline".into(), offline_graph("unplugged", 2)),
        (
            "healthy".into(),
            physical_graph(DeviceSelector::DefaultInput, 0.0),
        ),
    ];
    let composed = compile(&documents, &inventory(), &apps).unwrap();
    let healthy_source = &composed.native_sources[&physical_id("physical-one")];
    let healthy_output = &composed.native_outputs[&physical_id("physical-one")];
    let plan = Arc::new(RenderPlan::prepare(&composed.execution).unwrap());
    let sources = plan
        .source_bindings()
        .iter()
        .map(|binding| {
            vec![
                if binding.node_id() == healthy_source {
                    0.25
                } else {
                    0.0
                };
                8
            ]
        })
        .collect::<Vec<_>>();
    let mut sinks = vec![vec![0.0; 8]; plan.sink_bindings().len()];
    let mut processor = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    processor
        .process_block(
            4,
            &sources.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            &mut sinks.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
        )
        .unwrap();
    for (binding, samples) in plan.sink_bindings().iter().zip(sinks) {
        samples_equal(
            &samples,
            if binding.node_id() == healthy_output {
                0.25
            } else {
                0.0
            },
        );
    }
    let mut reconnected = inventory();
    reconnected.push(AudioDeviceDescriptor {
        uid: "unplugged".into(),
        display_name: "Reconnected".into(),
        input_channels: 2,
        output_channels: 2,
        is_default_input: false,
        is_default_output: false,
    });
    let online = compile(&documents, &reconnected, &apps).unwrap();
    assert_eq!(composed.native_sources, online.native_sources);
    assert_eq!(composed.native_outputs, online.native_outputs);
}

#[test]
fn zero_channel_direction_is_offline_but_nonzero_insufficient_width_is_invalid() {
    let apps = Apps(Cell::new(0));
    let documents = vec![("A".into(), offline_graph("physical-one", 2))];
    for (input, output) in [(0, 2), (2, 0), (0, 0)] {
        let mut devices = inventory();
        devices[0].input_channels = input;
        devices[0].output_channels = output;
        let composed = compile(&documents, &devices, &apps).unwrap();
        assert_eq!(composed.native_sources.len(), 1);
        assert_eq!(composed.native_outputs.len(), 1);
        devices.push(devices[0].clone());
        assert!(compile(&documents, &devices, &apps).is_err());
    }
    for (input, output) in [(1, 2), (2, 1)] {
        let mut devices = inventory();
        devices[0].input_channels = input;
        devices[0].output_channels = output;
        assert!(compile(&documents, &devices, &apps).is_err());
    }
}
