//! Graph serialization, identity and virtual endpoint contracts.
use super::*;

#[test]
fn installed_stereo_acceptance_uses_a_real_source_reference_and_recording_route() {
    let document: GraphDocument =
        serde_json::from_str(include_str!("../fixtures/qualification-stereo.json")).unwrap();
    let plan = GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 128,
                },
                ..context()
            },
        )
        .unwrap();
    let source = plan
        .nodes()
        .iter()
        .find(|node| node.id().as_str() == "qa-source")
        .unwrap();
    assert!(source.inputs().is_empty());
    assert_eq!(source.outputs()[0].id().as_str(), FROM_OS_PORT_ID);
    assert_eq!(source.outputs()[0].channels(), 2);
    let recorder = plan
        .nodes()
        .iter()
        .find(|node| node.id().as_str() == "qa-recorder")
        .unwrap();
    assert_eq!(recorder.inputs()[0].channels(), 2);
}

#[test]
fn node_control_defaults_round_trip_and_survives_compilation() {
    let mut document: GraphDocument =
        serde_json::from_str(include_str!("../fixtures/stereo-stream.json")).unwrap();
    assert!(
        document
            .nodes
            .iter()
            .all(|node| node.control == crate::NodeControl::default())
    );
    document.nodes[0].control = crate::NodeControl {
        enabled: false,
        muted: true,
    };
    let encoded = serde_json::to_string(&document).unwrap();
    assert_eq!(
        serde_json::from_str::<GraphDocument>(&encoded).unwrap(),
        document
    );
    let plan = GraphCompiler::new(BuiltInNodeResolver)
        .compile(&document, context())
        .unwrap();
    let compiled = plan
        .nodes()
        .iter()
        .find(|node| node.id() == &document.nodes[0].id)
        .unwrap();
    assert_eq!(compiled.control(), document.nodes[0].control);
}

#[test]
fn model_helpers_preserve_typed_ids_ports_kinds_and_defaults() {
    assert_eq!(NodeId::new("node").to_string(), "node");
    assert_eq!(EdgeId::new("edge").to_string(), "edge");
    assert_eq!(PortId::new("port").to_string(), "port");
    assert_eq!(
        NodeTypeId::new("io.mixrelay.compressor").to_string(),
        "io.mixrelay.compressor"
    );

    let selector: ApplicationSelector = serde_json::from_value(serde_json::json!({
        "match": "bundle_id",
        "bundle_id": "org.mozilla.firefox"
    }))
    .expect("selector should deserialize");
    assert!(matches!(
        selector,
        ApplicationSelector::BundleId {
            include_descendants: true,
            ..
        }
    ));

    let port = CompiledPort::new(PortId::from(AUDIO_PORT_ID), PortDirection::Output, 6);
    assert_eq!(port.id().as_str(), AUDIO_PORT_ID);
    assert_eq!(port.direction(), PortDirection::Output);
    assert_eq!(port.channels(), 6);

    let kinds = [
        app(ApplicationSelector::AllExcept { bundle_ids: vec![] }),
        device_input(DeviceSelector::DefaultInput, 2),
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: 2,
        }),
        mixer(vec![PortId::from("input")], 2),
        gain(2, 0.0),
        virtual_endpoint("uid", "name", Some(64), Some(2)),
        provider("io.mixrelay.compressor"),
    ];
    let actual: Vec<_> = kinds.iter().map(NodeConfig::kind).collect();
    assert_eq!(
        actual,
        [
            NodeKind::ApplicationCapture,
            NodeKind::AudioDeviceInput,
            NodeKind::AudioDeviceOutput,
            NodeKind::Mixer,
            NodeKind::Gain,
            NodeKind::VirtualEndpoint,
            NodeKind::Provider
        ]
    );
}

#[test]
fn host_audio_contracts_remain_valid_when_consumed_by_the_graph_crate() {
    let NodeConfig::VirtualEndpoint(endpoint) = virtual_endpoint("uid", "name", Some(64), Some(2))
    else {
        unreachable!("helper must produce a virtual endpoint");
    };
    endpoint
        .validate(&VirtualEndpointCapabilities {
            max_render_channels: 64,
            max_capture_channels: 64,
            supported_sample_rates: vec![48_000, 96_000],
            supports_duplex: true,
            supports_dynamic_creation: true,
        })
        .expect("endpoint should satisfy host capabilities");

    let request = ApplicationCaptureRequest {
        selector: ApplicationSelector::BundleId {
            bundle_id: "org.mozilla.firefox".into(),
            include_descendants: true,
        },
        stream: StreamSpec {
            channels: 1,
            channel_layout: ChannelLayout::Mono,
        },
        source_playback: SourcePlaybackPolicy::MuteWhileCaptured,
        forwarding_endpoint: Some(VirtualEndpointId::from("io.mixrelay.capture")),
        gain_db: 0.0,
        requires_reselection: false,
    };
    request
        .validate(&ApplicationCaptureCapabilities {
            max_channels: 2,
            supported_selectors: BTreeSet::from([ApplicationSelectorKind::BundleId]),
            supports_mute_while_captured: true,
        })
        .expect("application capture should satisfy host capabilities");
}

#[test]
fn virtual_gain_roundtrips_as_graph_state_without_changing_the_native_spec() {
    let original = virtual_endpoint("uid", "name", Some(64), Some(16));
    let serialized = serde_json::to_value(&original).unwrap();
    assert!(serialized["config"].get("gainDb").is_none());
    let mut restored: NodeConfig = serde_json::from_value(serialized).unwrap();
    let NodeConfig::VirtualEndpoint(config) = &mut restored else {
        unreachable!()
    };
    assert_eq!(config.gain_db.to_bits(), 0.0_f32.to_bits());
    let native = config.spec.clone();
    config.gain_db = -12.0;
    assert_eq!(config.spec, native);
    let saved = serde_json::to_value(&restored).unwrap();
    assert_eq!(saved["config"]["gainDb"], -12.0);
    let reloaded: NodeConfig = serde_json::from_value(saved).unwrap();
    assert_eq!(reloaded, restored);
}

fn resolve_virtual_reference(
    render_channels: Option<u16>,
    capture_channels: Option<u16>,
    reference: VirtualEndpointReference,
) -> Result<ResolvedNode, NodeResolutionError> {
    let mut config = virtual_endpoint("device", "Device", render_channels, capture_channels);
    let NodeConfig::VirtualEndpoint(endpoint) = &mut config else {
        unreachable!()
    };
    endpoint.reference = reference;
    BuiltInNodeResolver.resolve(&configured_node("reference", config), &context())
}

#[test]
fn virtual_references_require_visibility_and_reject_obsolete_signal_selection() {
    let config = virtual_endpoint("uid", "name", Some(64), Some(16));
    let serialized = serde_json::to_value(&config).unwrap();
    assert_eq!(
        serialized["config"]["reference"],
        serde_json::json!({"render":true,"capture":true})
    );
    for field in ["render", "capture"] {
        let mut invalid = serialized.clone();
        invalid["config"]["reference"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<NodeConfig>(invalid).is_err());
    }
    let mut obsolete = serialized;
    obsolete["config"]["reference"]["readSide"] = serde_json::json!("os_playback");
    assert!(serde_json::from_value::<NodeConfig>(obsolete).is_err());
}

#[test]
fn virtual_source_uses_driver_capture_width_and_one_public_port() {
    for (render, capture, channels) in [
        (Some(64), Some(16), 16),
        (Some(2), Some(16), 16),
        (None, Some(32), 32),
    ] {
        let resolved = resolve_virtual_reference(
            render,
            capture,
            VirtualEndpointReference {
                render: true,
                capture: false,
            },
        )
        .unwrap();
        assert!(resolved.inputs.is_empty());
        assert_eq!(
            resolved.outputs,
            vec![CompiledPort::new(
                PortId::from(FROM_OS_PORT_ID),
                PortDirection::Output,
                channels
            )]
        );
    }
}

#[test]
fn runtime_transport_cannot_be_serialized_or_deserialized_as_authored_graph() {
    let NodeConfig::VirtualEndpoint(config) = virtual_endpoint("id", "Name", Some(2), Some(16))
    else {
        unreachable!()
    };
    let transport = NodeConfig::VirtualEndpointTransport(config.spec);
    assert!(serde_json::to_value(&transport).is_err());
    assert!(
        serde_json::from_value::<NodeConfig>(serde_json::json!({
            "type":"virtual_endpoint_transport","config":{}
        }))
        .is_err()
    );
    let resolved = BuiltInNodeResolver
        .resolve(&configured_node("native", transport), &context())
        .unwrap();
    assert_eq!(resolved.outputs[0].channels(), 16);
    assert_eq!(resolved.inputs[0].channels(), 2);
}

#[test]
fn runtime_transport_memory_accounting_includes_reserved_native_spec_capacity() {
    let mut identity = String::with_capacity(128);
    identity.push_str("device-id");
    let mut name = String::with_capacity(256);
    name.push_str("Device");
    let mut rates = Vec::with_capacity(32);
    rates.push(48_000);
    let expected = identity.capacity() + name.capacity() + rates.capacity() * size_of::<u32>();
    let spec = VirtualEndpointSpec {
        id: VirtualEndpointId::new(identity),
        display_name: name,
        render: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        capture: None,
        supported_sample_rates: rates,
        default_sample_rate: 48_000,
    };
    assert_eq!(spec.owned_heap_capacity_bytes(), expected);
}

#[test]
fn virtual_sink_reference_does_not_expose_unselected_native_playback() {
    let resolved = resolve_virtual_reference(
        Some(64),
        Some(16),
        VirtualEndpointReference {
            render: false,
            capture: true,
        },
    )
    .unwrap();
    assert!(resolved.outputs.is_empty());
    assert_eq!(
        resolved.inputs,
        vec![CompiledPort::new(
            PortId::from(TO_OS_PORT_ID),
            PortDirection::Input,
            64,
        )]
    );
}

#[test]
fn capture_only_device_can_expose_its_produced_mix_without_native_render() {
    let resolved = resolve_virtual_reference(
        None,
        Some(32),
        VirtualEndpointReference {
            render: true,
            capture: false,
        },
    )
    .unwrap();
    assert!(resolved.inputs.is_empty());
    assert_eq!(
        resolved.outputs,
        vec![CompiledPort::new(
            PortId::from(FROM_OS_PORT_ID),
            PortDirection::Output,
            32,
        )]
    );
}

#[test]
fn virtual_reference_rejects_no_direction_or_missing_native_sink() {
    for (render, capture, native_render, native_capture) in [
        (false, false, Some(2), Some(2)),
        (false, true, None, Some(2)),
        (true, false, Some(2), None),
        (true, false, None, None),
    ] {
        assert!(
            resolve_virtual_reference(
                native_render,
                native_capture,
                VirtualEndpointReference { render, capture }
            )
            .is_err()
        );
    }
}

#[test]
fn native_spec_conversion_selects_only_available_directions() {
    for (render, capture) in [(Some(2), None), (None, Some(16)), (Some(64), Some(32))] {
        let NodeConfig::VirtualEndpoint(config) = virtual_endpoint("uid", "name", render, capture)
        else {
            unreachable!()
        };
        assert_eq!(
            config.reference,
            VirtualEndpointReference {
                render: capture.is_some(),
                capture: render.is_some(),
            }
        );
    }
}

#[test]
fn virtual_gain_rejects_invalid_values_before_audio_preparation() {
    for gain in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -60.01,
        12.01,
        1000.0,
    ] {
        assert!(validate_virtual_endpoint_gain(gain).is_err());
        let mut config = virtual_endpoint("uid", "name", Some(2), Some(2));
        let NodeConfig::VirtualEndpoint(endpoint) = &mut config else {
            unreachable!()
        };
        endpoint.gain_db = gain;
        assert!(
            BuiltInNodeResolver
                .resolve(&configured_node("uid", config), &context())
                .is_err()
        );
    }
    for gain in [-60.0, 0.0, 12.0] {
        assert!(validate_virtual_endpoint_gain(gain).is_ok());
        let mut config = virtual_endpoint("uid", "name", Some(2), Some(2));
        let NodeConfig::VirtualEndpoint(endpoint) = &mut config else {
            unreachable!()
        };
        endpoint.gain_db = gain;
        assert!(
            BuiltInNodeResolver
                .resolve(&configured_node("uid", config), &context())
                .is_ok()
        );
    }
}
