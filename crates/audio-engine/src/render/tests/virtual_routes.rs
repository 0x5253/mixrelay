//! Virtual endpoint routing, source gain and independent-source metering.
use super::*;

#[test]
fn processes_a_duplex_virtual_endpoint_through_gain_without_a_false_cycle() {
    let execution = compile(
        vec![
            virtual_endpoint("virtual"),
            node(
                "gain",
                NodeConfig::Gain(GainConfig {
                    channels: 2,
                    gain_db: -6.020_600_3,
                }),
            ),
        ],
        vec![
            edge(
                "from-os-to-gain",
                "virtual",
                FROM_OS_PORT_ID,
                "gain",
                AUDIO_PORT_ID,
            ),
            edge(
                "gain-to-os",
                "gain",
                AUDIO_PORT_ID,
                "virtual",
                TO_OS_PORT_ID,
            ),
        ],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).expect("gain path should lower"));
    let mut processor = BlockProcessor::new(plan).unwrap();
    let source = [1.0, -1.0, 0.5, -0.5];
    let mut sink = [0.0; 4];

    processor
        .process_block(2, &[&source], &mut [&mut sink])
        .expect("gain path should process");

    for (actual, expected) in sink.into_iter().zip([0.5, -0.5, 0.25, -0.25]) {
        assert!((actual - expected).abs() < 0.000_001);
    }
}

#[test]
fn virtual_source_gain_scales_pcm_and_both_meter_banks_once_without_allocations() {
    for channels in [2_u16, 8, 16, 32, 64] {
        let mut endpoint = virtual_endpoint("virtual");
        let NodeConfig::VirtualEndpointTransport(config) = &mut endpoint.config else {
            unreachable!();
        };
        let stream = StreamSpec {
            channels,
            channel_layout: ChannelLayout::Discrete,
        };
        config.render = Some(stream);
        config.capture = Some(stream);
        let execution = compile(
            vec![
                endpoint,
                node(
                    "local-gain",
                    NodeConfig::Gain(GainConfig {
                        channels,
                        gain_db: -6.020_600_3,
                    }),
                ),
            ],
            vec![
                edge(
                    "read",
                    "virtual",
                    FROM_OS_PORT_ID,
                    "local-gain",
                    AUDIO_PORT_ID,
                ),
                edge(
                    "write",
                    "local-gain",
                    AUDIO_PORT_ID,
                    "virtual",
                    TO_OS_PORT_ID,
                ),
            ],
        );
        let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
        let meters = Arc::new(crate::PeakMeterBank::new(plan.meter_bindings().len(), 64));
        let mut processor =
            BlockProcessor::with_peak_meters(Arc::clone(&plan), Arc::clone(&meters)).unwrap();
        let source: Vec<f32> = (0..usize::from(channels) * 2)
            .map(|index| if index % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut sink = vec![0.0; source.len()];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        for _ in 0..100 {
            processor
                .process_block(2, &[&source], &mut [&mut sink])
                .unwrap();
        }
        let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
        assert_eq!(operations, Some(0));
        for (input, output) in source.iter().zip(sink) {
            assert!((output - input * 0.5).abs() < 0.000_001);
        }
        for (index, binding) in plan.meter_bindings().iter().enumerate() {
            let expected = if binding.port_id().as_str() == FROM_OS_PORT_ID {
                1.0
            } else {
                0.5
            };
            for peak in meters.take_peaks(1, index, channels) {
                assert!((peak - expected).abs() < 0.000_001);
            }
        }
    }
}

#[test]
fn virtual_source_gain_does_not_attenuate_an_independent_application_capture_path() {
    let mut endpoint = virtual_endpoint("virtual");
    let NodeConfig::VirtualEndpointTransport(config) = &mut endpoint.config else {
        unreachable!();
    };
    let _ = config;
    let execution = compile(
        vec![
            endpoint,
            app_source("app"),
            node(
                "local-gain",
                NodeConfig::Gain(GainConfig {
                    channels: 2,
                    gain_db: -12.0,
                }),
            ),
        ],
        vec![
            edge(
                "source-gain",
                "virtual",
                FROM_OS_PORT_ID,
                "local-gain",
                AUDIO_PORT_ID,
            ),
            edge("app-to-os", "app", AUDIO_PORT_ID, "virtual", TO_OS_PORT_ID),
        ],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let app = [1.0; 4];
    let virtual_input = [2.0; 4];
    let sources: Vec<&[f32]> = plan
        .source_bindings()
        .iter()
        .map(|binding| {
            if binding.port_id().as_str() == FROM_OS_PORT_ID {
                virtual_input.as_slice()
            } else {
                app.as_slice()
            }
        })
        .collect();
    let mut processor = BlockProcessor::new(plan).unwrap();
    let mut output = [0.0; 4];
    processor
        .process_block(2, &sources, &mut [&mut output])
        .unwrap();
    for sample in output {
        assert!((sample - 1.0).abs() < f32::EPSILON);
    }
}

#[test]
fn unconnected_application_and_hardware_sources_are_drained_and_metered() {
    let execution = compile(
        vec![app_source("app"), device_input("mic"), device_output("out")],
        vec![],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    assert_eq!(plan.source_bindings().len(), 2);
    assert!(plan.sink_bindings().is_empty());
    let meters = Arc::new(crate::PeakMeterBank::new(plan.meter_bindings().len(), 64));
    let mut processor = BlockProcessor::with_peak_meters(plan.clone(), meters.clone()).unwrap();
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    processor
        .process_block(
            2,
            &[&[0.5, -0.25, 0.1, 0.2], &[0.75, -0.5, 0.2, 0.3]],
            &mut [],
        )
        .unwrap();
    assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
    for (index, binding) in plan.meter_bindings().iter().enumerate() {
        let peaks = meters.take_peaks(1, index, 2);
        let expected = match binding.node_id().as_str() {
            "app" => [0.5, 0.25],
            "mic" => [0.75, 0.5],
            "out" => [0.0, 0.0],
            _ => unreachable!(),
        };
        assert_eq!(peaks, expected);
    }
}

#[test]
fn supports_render_only_virtual_endpoints() {
    let render_only = node(
        "render-only",
        NodeConfig::VirtualEndpointTransport(VirtualEndpointSpec {
            id: VirtualEndpointId::from("render-only"),
            display_name: "render-only".into(),
            render: Some(stereo_stream()),
            capture: None,
            supported_sample_rates: vec![48_000],
            default_sample_rate: 48_000,
        }),
    );
    RenderPlan::prepare(&compile(vec![render_only], vec![]))
        .expect("render-only endpoint should lower");
}

#[test]
fn supports_render_only_contribution_sink_with_no_capture_source() {
    let capture_only = NodeConfig::VirtualEndpointTransport(VirtualEndpointSpec {
        id: VirtualEndpointId::from("capture-only"),
        display_name: "Capture only".into(),
        render: Some(stereo_stream()),
        capture: None,
        supported_sample_rates: vec![48_000],
        default_sample_rate: 48_000,
    });
    let execution = compile(vec![node("capture-only", capture_only)], vec![]);
    let plan = Arc::new(RenderPlan::prepare(&execution).expect("plan should lower"));
    assert!(plan.source_bindings().is_empty());
    assert_eq!(plan.sink_bindings().len(), 1);
    let mut processor = BlockProcessor::new(plan).unwrap();
    let mut sink = [1.0; 4];

    processor
        .process_block(2, &[], &mut [&mut sink])
        .expect("capture-only endpoint should render silence");
    assert!(sink.iter().all(|sample| sample.abs() < f32::EPSILON));
}

#[test]
fn routes_virtual_endpoint_render_to_capture_inside_one_node() {
    let execution = compile(
        vec![virtual_endpoint("pass-through")],
        vec![edge(
            "internal",
            "pass-through",
            FROM_OS_PORT_ID,
            "pass-through",
            TO_OS_PORT_ID,
        )],
    );
    let plan = Arc::new(RenderPlan::prepare(&execution).expect("pass-through should lower"));
    let input = [1.0, -1.0, 0.5, -0.5];
    let mut output = [0.0; 4];
    let mut processor = BlockProcessor::new(plan).unwrap();
    processor
        .process_block(2, &[&input], &mut [&mut output])
        .expect("internal route should process");
    for (actual, expected) in output.into_iter().zip(input) {
        assert!((actual - expected).abs() < f32::EPSILON);
    }
}
