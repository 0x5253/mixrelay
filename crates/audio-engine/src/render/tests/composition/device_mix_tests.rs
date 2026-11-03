use super::*;

#[test]
fn native_clients_follow_all_graphs_direction_union_without_changing_driver_geometry() {
    let canonical = device("X");
    let registry = DeviceRegistry::new(vec![canonical.clone()]).unwrap();
    let document = GraphDocument::new(1, vec![], vec![]);
    let readers = [reference("read", "X", true, false)];
    let writers = [reference("write", "X", false, true)];
    for (read, write) in [(true, false), (false, true), (true, true)] {
        let mut graphs = Vec::new();
        for (enabled, id, references) in [
            (read, "reader", readers.as_slice()),
            (write, "writer", writers.as_slice()),
        ] {
            if enabled {
                graphs.push(GraphFragment {
                    id,
                    document: &document,
                    device_references: references,
                    source_references: &[],
                    output_references: &[],
                });
            }
        }
        let composed =
            compose(&registry, &graphs, 1, context(), &GraphCompiler::default()).unwrap();
        let plan = RenderPlan::prepare(&composed.execution).unwrap();
        assert_eq!(plan.source_bindings().len(), usize::from(read));
        assert_eq!(plan.sink_bindings().len(), usize::from(write));
        for binding in plan.source_bindings().iter().chain(plan.sink_bindings()) {
            assert_eq!(binding.channels(), 2);
            assert_eq!(binding.virtual_endpoint_id(), Some(&canonical.id));
        }
        assert_eq!(registry.get(&canonical.id), Some(&canonical));
    }
}

fn asymmetric_registry() -> DeviceRegistry {
    let wide = StreamSpec {
        channels: 4,
        channel_layout: ChannelLayout::Discrete,
    };
    let mut x = device("X");
    x.render = Some(wide);
    x.capture = Some(wide);
    let mut y = device("Y");
    y.render = Some(wide);
    y.capture = None;
    DeviceRegistry::new(vec![x, y])
        .unwrap()
        .with_sources(vec![SharedSource {
            id: "input".into(),
            config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::DefaultInput,
                output_channels: 4,
                gain_db: 0.0,
            }),
        }])
        .unwrap()
}

fn asymmetric_composition() -> ComposedGraph {
    let registry = asymmetric_registry();
    let source = [SourceReference {
        control: mixrelay_audio_graph::NodeControl::default(),
        node_id: "input".into(),
        source_id: "input".into(),
        channels: 4,
        gain_db: 0.0,
        forwarding_endpoint: None,
    }];
    let writer = GraphDocument::new(
        1,
        vec![],
        vec![edge("send", "input", AUDIO_PORT_ID, "x", TO_OS_PORT_ID)],
    );
    let reader = GraphDocument::new(
        1,
        vec![],
        vec![edge("read", "x", FROM_OS_PORT_ID, "y", TO_OS_PORT_ID)],
    );
    let write_refs = [DeviceReference {
        control: mixrelay_audio_graph::NodeControl::default(),
        node_id: "x".into(),
        device_id: "X".into(),
        read: None,
        write_channels: Some(4),
        gain_db: 0.0,
    }];
    let read_refs = [
        DeviceReference {
            control: mixrelay_audio_graph::NodeControl::default(),
            node_id: "x".into(),
            device_id: "X".into(),
            read: Some(DeviceReadReference { channels: 4 }),
            write_channels: None,
            gain_db: -6.020_600_3,
        },
        DeviceReference {
            control: mixrelay_audio_graph::NodeControl::default(),
            node_id: "y".into(),
            device_id: "Y".into(),
            read: None,
            write_channels: Some(4),
            gain_db: 0.0,
        },
    ];
    compose(
        &registry,
        &[
            GraphFragment {
                id: "A",
                document: &writer,
                device_references: &write_refs,
                source_references: &source,
                output_references: &[],
            },
            GraphFragment {
                id: "B",
                document: &reader,
                device_references: &read_refs,
                source_references: &[],
                output_references: &[],
            },
        ],
        1,
        context(),
        &GraphCompiler::default(),
    )
    .unwrap()
}

#[test]
fn asymmetric_native_geometry_sums_all_channels_before_per_reference_gain() {
    let composed = asymmetric_composition();
    let plan = Arc::new(RenderPlan::prepare(&composed.execution).unwrap());
    let inputs: Vec<Vec<f32>> = plan
        .source_bindings()
        .iter()
        .map(|binding| {
            if binding.virtual_endpoint_id().is_some() {
                assert_eq!(binding.channels(), 4);
                [1.0, 1.0, 0.6, 0.8].repeat(4)
            } else {
                [0.2, 0.4, 0.6, 0.8].repeat(4)
            }
        })
        .collect();
    let mut outputs: Vec<Vec<f32>> = plan
        .sink_bindings()
        .iter()
        .map(|binding| {
            assert_eq!(binding.channels(), 4);
            vec![0.0; 16]
        })
        .collect();
    let mut processor = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    for cycle in 0..2 {
        processor
            .process_block(
                4,
                &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                &mut outputs
                    .iter_mut()
                    .map(Vec::as_mut_slice)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        for (binding, samples) in plan.sink_bindings().iter().zip(&outputs) {
            let expected = if binding.virtual_endpoint_id().unwrap().as_str() == "X" {
                [0.2, 0.4, 0.6, 0.8]
            } else if cycle == 0 {
                [0.0; 4]
            } else {
                [0.5, 0.5, 0.3, 0.4]
            };
            for frame in samples.chunks_exact(4) {
                for (sample, value) in frame.iter().zip(expected) {
                    assert!((sample - value).abs() < 0.000_001);
                }
            }
        }
    }
    assert_eq!(composed.native_devices.len(), 2);
    assert_eq!(composed.device_buses.len(), 1);
}

#[test]
fn explicit_driver_feedback_is_finite_and_off_or_mute_stops_only_contributions() {
    let registry = DeviceRegistry::new(vec![device("X")]).unwrap();
    let document = GraphDocument::new(
        1,
        vec![],
        vec![edge("feedback", "x", FROM_OS_PORT_ID, "x", TO_OS_PORT_ID)],
    );
    for (enabled, muted) in [(true, false), (true, true), (false, false)] {
        let mut local = reference("x", "X", true, true);
        local.control = mixrelay_audio_graph::NodeControl { enabled, muted };
        let composed = compose(
            &registry,
            &[GraphFragment {
                id: "feedback",
                document: &document,
                device_references: &[local],
                source_references: &[],
                output_references: &[],
            }],
            1,
            context(),
            &GraphCompiler::default(),
        )
        .unwrap();
        let mut processor =
            BlockProcessor::new(Arc::new(RenderPlan::prepare(&composed.execution).unwrap()))
                .unwrap();
        let mut capture = [1.0; 8];
        let mut contribution = [0.0; 8];
        for expected in [0.0, 1.0, 1.0, 2.0, 2.0] {
            HEAP_OPERATIONS.with(|count| count.set(Some(0)));
            {
                processor
                    .process_block(4, &[&capture], &mut [&mut contribution])
                    .unwrap();
            }
            assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
            let expected = if enabled && !muted { expected } else { 0.0 };
            assert!(
                contribution
                    .iter()
                    .all(|sample| (*sample - expected).abs() < 0.000_001)
            );
            // The native Driver performs this sum in its own callback. Feeding the
            // next observed capture cannot execute graph work recursively in this block.
            for (mixed, graph) in capture.iter_mut().zip(contribution) {
                *mixed = 1.0 + graph;
            }
        }
        assert_eq!(registry.devices().count(), 1);
    }
}
