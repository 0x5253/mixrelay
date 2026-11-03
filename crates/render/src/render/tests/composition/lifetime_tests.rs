use super::*;

#[test]
fn virtual_device_survives_other_graph_reference_removal_but_not_final_removal() {
    let registry = mic_registry(vec![device("X")]);
    let document = source_document();
    let devices = [reference("shared", "X", false, true)];
    let sources_a = [mic_reference(0.0)];
    let sources_b = [mic_reference(-6.020_6)];
    let make = |id, sources| GraphFragment {
        id,
        document: &document,
        device_references: &devices,
        source_references: sources,
        output_references: &[],
    };
    let both = compose(
        &registry,
        &[make("A", &sources_a), make("B", &sources_b)],
        1,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    let remaining = compose(
        &registry,
        &[make("B", &sources_b)],
        2,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap();
    assert_eq!(both.native_devices, remaining.native_devices);
    assert_frame(
        &pcm(&both)[&both.native_devices[&VirtualEndpointId::from("X")]],
        [0.3, 0.6],
    );
    assert_frame(
        &pcm(&remaining)[&remaining.native_devices[&VirtualEndpointId::from("X")]],
        [0.1, 0.2],
    );
    let empty = compose(&registry, &[], 3, context(), &GraphCompiler::default()).unwrap();
    assert!(empty.native_devices.is_empty());
    assert!(empty.device_buses.is_empty());
    assert!(empty.native_sources.is_empty());
}

#[test]
fn virtual_gain_contract_is_identical_for_authored_and_direct_reference_lowering() {
    let registry = DeviceRegistry::new(vec![device("X")]).unwrap();
    for (gain, accepted) in [
        (-60.0, true),
        (12.0, true),
        (-60.01, false),
        (12.01, false),
        (1000.0, false),
        (f32::NAN, false),
        (f32::INFINITY, false),
        (f32::NEG_INFINITY, false),
    ] {
        let empty = GraphDocument::new(1, vec![], vec![]);
        let mut view = reference("endpoint", "X", true, false);
        view.gain_db = gain;
        let direct = compose(
            &registry,
            &[GraphFragment {
                id: "A",
                document: &empty,
                device_references: &[view],
                source_references: &[],
                output_references: &[],
            }],
            2,
            context(),
            &GraphCompiler::default(),
        );
        assert_eq!(direct.is_ok(), accepted, "direct gain {gain}");
        let mut config: mixrelay_audio_graph::VirtualEndpointConfig = device("X").into();
        config.gain_db = gain;
        let document = GraphDocument::new(
            1,
            vec![NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: NodeId::from("endpoint"),
                label: None,
                config: NodeConfig::VirtualEndpoint(config),
            }],
            vec![],
        );
        let authored = lower_document(&document, &registry, &ResolvedNodeIdentities::default());
        assert_eq!(authored.is_ok(), accepted, "authored gain {gain}");
    }
}
