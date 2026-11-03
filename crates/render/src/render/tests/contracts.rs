//! Compiled binding geometry and malformed-plan rejection.
use super::*;

#[test]
fn external_bindings_keep_device_identity_separate_from_graph_node_names() {
    let mut endpoint = virtual_endpoint("canonical-device");
    endpoint.id = NodeId::from("graph-b::local-reference");
    let execution = compile(
        vec![endpoint, app_source("application"), device_input("mic")],
        vec![],
    );
    let plan = RenderPlan::prepare(&execution).expect("valid independent references");
    for binding in plan.source_bindings().iter().chain(plan.sink_bindings()) {
        let expected = (binding.node_id().as_str() == "graph-b::local-reference")
            .then_some("canonical-device");
        assert_eq!(
            binding.virtual_endpoint_id().map(VirtualEndpointId::as_str),
            expected
        );
    }
}

#[test]
fn native_transport_bindings_keep_asymmetric_device_geometry() {
    let mut endpoint = virtual_endpoint("device");
    if let NodeConfig::VirtualEndpointTransport(spec) = &mut endpoint.config {
        spec.capture = Some(StreamSpec {
            channels: 16,
            channel_layout: ChannelLayout::Discrete,
        });
    }
    let plan = RenderPlan::prepare(&compile(vec![endpoint], vec![])).unwrap();
    assert_eq!(plan.source_bindings()[0].channels(), 16);
    assert_eq!(plan.sink_bindings()[0].channels(), 2);
}

#[test]
fn authored_virtual_source_must_be_composed_before_rendering() {
    let mut endpoint = virtual_endpoint("device");
    if let NodeConfig::VirtualEndpointTransport(spec) = &endpoint.config {
        endpoint.config = NodeConfig::VirtualEndpoint(spec.clone().into());
    }
    let execution = compile(vec![endpoint], vec![]);
    assert!(matches!(RenderPlan::prepare(&execution),
        Err(RenderPlanError::UnloweredVirtualEndpoint {node_id}) if node_id.as_str()=="device"));
}

#[test]
fn rejects_invalid_block_shapes_before_writing_outputs() {
    let plan = Arc::new(RenderPlan::prepare(&processor_graph()).expect("plan should lower"));
    let mut processor = BlockProcessor::new(plan).unwrap();
    let full = [0.0; 8];
    let sources: [&[f32]; 3] = [&full, &full, &full];
    let mut first = [9.0; 8];
    let mut second = [9.0; 8];

    assert_eq!(
        processor.process_block(5, &sources, &mut [&mut first, &mut second]),
        Err(ProcessBlockError::FrameCountExceeded {
            maximum: 4,
            found: 5,
        })
    );
    assert_eq!(
        processor.process_block(2, &sources[..2], &mut [&mut first, &mut second]),
        Err(ProcessBlockError::SourceCountMismatch {
            expected: 3,
            found: 2,
        })
    );
    assert_eq!(
        processor.process_block(2, &sources, &mut [&mut first]),
        Err(ProcessBlockError::SinkCountMismatch {
            expected: 2,
            found: 1,
        })
    );
    let short = [0.0; 3];
    let short_sources: [&[f32]; 3] = [&short, &full, &full];
    assert_eq!(
        processor.process_block(2, &short_sources, &mut [&mut first, &mut second]),
        Err(ProcessBlockError::SourceBufferTooShort {
            index: 0,
            required: 4,
            found: 3,
        })
    );
    let mut short_sink = [0.0; 3];
    assert_eq!(
        processor.process_block(2, &sources, &mut [&mut short_sink, &mut second]),
        Err(ProcessBlockError::SinkBufferTooShort {
            index: 0,
            required: 4,
            found: 3,
        })
    );
    assert!(
        first
            .into_iter()
            .all(|sample| (sample - 9.0).abs() < f32::EPSILON)
    );
    assert!(
        second
            .into_iter()
            .all(|sample| (sample - 9.0).abs() < f32::EPSILON)
    );
}

#[test]
fn rejects_custom_resolver_shapes_that_cannot_render_safely() {
    let missing_port = |config, shape, port_id| {
        assert_eq!(
            RenderPlan::prepare(&custom_plan(config, shape))
                .expect_err("required port must be present"),
            RenderPlanError::MissingPort {
                node_id: NodeId::from("custom"),
                port_id: PortId::from(port_id),
            }
        );
    };

    missing_port(
        app_source("custom").config,
        ResolverShape::MissingSourceOutput,
        AUDIO_PORT_ID,
    );
    missing_port(
        device_output("custom").config,
        ResolverShape::MissingSinkInput,
        AUDIO_PORT_ID,
    );
    missing_port(
        NodeConfig::Mixer(MixerConfig {
            inputs: vec![PortId::from("one")],
            channels: 2,
        }),
        ResolverShape::MissingMixerInput,
        "one",
    );
    missing_port(
        NodeConfig::Mixer(MixerConfig {
            inputs: vec![PortId::from("one")],
            channels: 2,
        }),
        ResolverShape::MissingMixerOutput,
        AUDIO_PORT_ID,
    );
    let gain = NodeConfig::Gain(GainConfig {
        channels: 2,
        gain_db: 0.0,
    });
    missing_port(gain.clone(), ResolverShape::MissingGainInput, AUDIO_PORT_ID);
    missing_port(gain, ResolverShape::MissingGainOutput, AUDIO_PORT_ID);
    missing_port(
        virtual_endpoint("custom").config,
        ResolverShape::MissingVirtualRender,
        FROM_OS_PORT_ID,
    );
    let capture_only = NodeConfig::VirtualEndpointTransport(VirtualEndpointSpec {
        id: VirtualEndpointId::from("custom"),
        display_name: "custom".into(),
        render: Some(stereo_stream()),
        capture: None,
        supported_sample_rates: vec![48_000],
        default_sample_rate: 48_000,
    });
    missing_port(
        capture_only,
        ResolverShape::MissingVirtualCapture,
        TO_OS_PORT_ID,
    );
}

#[test]
fn rejects_unregistered_or_ambiguous_provider_kernels() {
    let provider = NodeConfig::Provider(ProviderNodeConfig {
        ports: None,
        type_id: "io.mixrelay.test".into(),
        parameters: serde_json::Value::Null,
    });
    assert_eq!(
        RenderPlan::prepare(&custom_plan(
            provider.clone(),
            ResolverShape::DuplicateProviderPort,
        ))
        .expect_err("duplicate ports are ambiguous"),
        RenderPlanError::DuplicatePort {
            node_id: NodeId::from("custom"),
            port_id: PortId::from(AUDIO_PORT_ID),
        }
    );
    assert_eq!(
        RenderPlan::prepare(&custom_plan(provider, ResolverShape::ValidProvider))
            .expect_err("provider kernel must be registered"),
        RenderPlanError::UnsupportedProvider {
            node_id: NodeId::from("custom"),
        }
    );
}

#[test]
fn rejects_render_arena_size_overflow() {
    let stereo = compile(vec![app_source("stereo")], vec![]);
    let multiply_error =
        allocate_slots(stereo.nodes(), usize::MAX).expect_err("stereo slot size must overflow");
    assert_eq!(multiply_error, RenderPlanError::BufferSizeOverflow);

    let mono = |id| {
        node(
            id,
            NodeConfig::ApplicationCapture(ApplicationCaptureRequest {
                selector: ApplicationSelector::BundleId {
                    bundle_id: format!("io.mixrelay.{id}"),
                    include_descendants: true,
                },
                stream: StreamSpec {
                    channels: 1,
                    channel_layout: ChannelLayout::Mono,
                },
                source_playback: SourcePlaybackPolicy::Preserve,
                forwarding_endpoint: None,
                gain_db: 0.0,
                requires_reselection: false,
            }),
        )
    };
    let two_mono = compile(vec![mono("first"), mono("second")], vec![]);
    let add_error = allocate_slots(two_mono.nodes(), usize::MAX)
        .expect_err("second mono slot offset must overflow");
    assert_eq!(add_error, RenderPlanError::BufferSizeOverflow);
}
