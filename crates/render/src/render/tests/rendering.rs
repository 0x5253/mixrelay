//! Prepared kernels, channel routing and hosted-provider PCM execution.
use super::*;

#[test]
fn lowers_bindings_and_processes_gain_mix_and_channel_routes() {
    let plan = Arc::new(RenderPlan::prepare(&processor_graph()).expect("plan should lower"));
    assert_eq!(plan.revision(), 1);
    assert_eq!(plan.format(), FORMAT);
    assert_eq!(plan.arena_samples(), 80);
    assert_eq!(
        plan.source_bindings()
            .iter()
            .map(|binding| (
                binding.node_id().as_str(),
                binding.port_id().as_str(),
                binding.channels()
            ))
            .collect::<Vec<_>>(),
        [
            ("app", AUDIO_PORT_ID, 2),
            ("mic", AUDIO_PORT_ID, 2),
            ("virtual", FROM_OS_PORT_ID, 2),
        ]
    );
    assert_eq!(
        plan.sink_bindings()
            .iter()
            .map(|binding| (binding.node_id().as_str(), binding.port_id().as_str()))
            .collect::<Vec<_>>(),
        [("monitor", AUDIO_PORT_ID), ("virtual", TO_OS_PORT_ID)]
    );

    let app = [1.0, 2.0, 3.0, 4.0];
    let mic = [0.5, 0.5, 0.5, 0.5];
    let from_os = [10.0, 20.0, 30.0, 40.0];
    let sources: [&[f32]; 3] = [&app, &mic, &from_os];
    let mut to_os = [0.0; 4];
    let mut monitor = [0.0; 4];
    let mut sinks: [&mut [f32]; 2] = [&mut monitor, &mut to_os];
    let mut processor = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    assert!(Arc::ptr_eq(processor.plan(), &plan));
    processor
        .process_block(2, &sources, &mut sinks)
        .expect("block should process");

    for (actual, expected) in monitor.into_iter().zip([20.0, 10.0, 40.0, 30.0]) {
        assert!((actual - expected).abs() < 0.000_001);
    }
    for (actual, expected) in to_os.into_iter().zip([0.75, 1.25, 1.75, 2.25]) {
        assert!((actual - expected).abs() < 0.000_001);
    }
}

#[test]
fn hosted_provider_uses_prepared_instance_in_the_render_plan() {
    let document = GraphDocument::new(
        1,
        vec![
            app_source("app"),
            node(
                "provider",
                NodeConfig::Provider(ProviderNodeConfig {
                    ports: None,
                    type_id: mixrelay_audio_graph::NodeTypeId::from("test:double"),
                    parameters: serde_json::json!({}),
                }),
            ),
            device_output("speaker"),
        ],
        vec![
            edge(
                "app-provider",
                "app",
                AUDIO_PORT_ID,
                "provider",
                AUDIO_PORT_ID,
            ),
            edge(
                "provider-speaker",
                "provider",
                AUDIO_PORT_ID,
                "speaker",
                AUDIO_PORT_ID,
            ),
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
        .expect("provider graph should compile");
    let meters = Arc::new(crate::PeakMeterBank::new(8, 8));
    let mut processor =
        BlockProcessor::prepare_with_hosted_processors(&execution, meters, &TestProviderFactory)
            .expect("provider should prepare");
    let source = [1.0, -1.0, 0.25, -0.25];
    let mut sink = [0.0; 4];
    processor
        .process_block(2, &[&source], &mut [&mut sink])
        .expect("provider block should process");
    for (actual, expected) in sink.iter().zip([2.0, -2.0, 0.5, -0.5]) {
        assert!((actual - expected).abs() <= f32::EPSILON);
    }
}
