use super::*;
use crate::{
    NodeControl, PortId, ProviderBus, ProviderBusKind, ProviderChannelFormat, ProviderPorts,
    ProviderPortsError,
};

fn bus(
    id: &str,
    name: &str,
    kind: ProviderBusKind,
    channels: u16,
    format: ProviderChannelFormat,
) -> ProviderBus {
    ProviderBus {
        id: PortId::from(id),
        name: name.to_owned(),
        kind,
        channels,
        channel_format: format,
    }
}

fn dormant(ports: Option<ProviderPorts>) -> NodeSpec {
    NodeSpec {
        id: "missing".into(),
        label: None,
        control: NodeControl {
            enabled: false,
            muted: true,
        },
        config: NodeConfig::Provider(ProviderNodeConfig {
            type_id: "unavailable:fixture".into(),
            ports,
            parameters: serde_json::json!({"preserved":42}),
        }),
    }
}

#[test]
fn off_unavailable_provider_preserves_saved_ports_routes_and_controls() {
    let off = dormant(Some(ProviderPorts::single_bus("test-layout", 2, 2)));
    let document = GraphDocument::new(
        1,
        vec![gain_node("source"), off.clone(), gain_node("sink")],
        vec![
            edge("in", "source", "missing"),
            edge("out", "missing", "sink"),
        ],
    );
    let execution = GraphCompiler::default()
        .compile(&document, context())
        .unwrap();
    let node = execution
        .nodes()
        .iter()
        .find(|node| node.id() == &off.id)
        .unwrap();
    assert_eq!(node.control(), off.control);
    assert_eq!(node.config(), &off.config);
    assert_eq!(node.inputs()[0].id().as_str(), "audio");
    assert_eq!(node.inputs()[0].channels(), 2);
    assert_eq!(node.outputs()[0].id().as_str(), "audio");
    assert_eq!(node.outputs()[0].channels(), 2);
    assert_eq!(execution.edges().len(), 2);
}

#[test]
fn off_provider_without_saved_ports_is_inert_and_on_still_requires_capability() {
    let mut document = GraphDocument::new(1, vec![dormant(None)], vec![]);
    let execution = GraphCompiler::default()
        .compile(&document, context())
        .unwrap();
    let node = &execution.nodes()[0];
    assert!(node.inputs().is_empty());
    assert!(node.outputs().is_empty());
    assert_eq!(node.config(), &document.nodes[0].config);
    for ports in [None, Some(ProviderPorts::single_bus("test-layout", 2, 2))] {
        document.nodes[0] = dormant(ports);
        document.nodes[0].control.enabled = true;
        let error = GraphCompiler::default()
            .compile(&document, context())
            .unwrap_err();
        assert!(matches!(
            error.issues(),
            [GraphIssue::NodeResolutionFailed { .. }]
        ));
    }
}

#[test]
fn invalid_saved_ports_are_not_accepted_for_dormant_provider() {
    for (inputs, outputs) in [(0, 2), (2, 0), (65, 2), (2, 65)] {
        let document = GraphDocument::new(
            1,
            vec![dormant(Some(ProviderPorts::single_bus(
                "test-layout",
                inputs,
                outputs,
            )))],
            vec![],
        );
        let error = GraphCompiler::default()
            .compile(&document, context())
            .unwrap_err();
        assert!(matches!(
            error.issues(),
            [GraphIssue::NodeResolutionFailed { .. }]
        ));
    }
}

#[test]
fn dormant_multibus_layout_compiles_each_port_in_saved_order() {
    let ports = ProviderPorts {
        layout_id: "fixture-layout-v1".into(),
        inputs: vec![
            bus(
                "audio",
                "Main",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            ),
            bus(
                "bus:1",
                "Sidechain",
                ProviderBusKind::Sidechain,
                1,
                ProviderChannelFormat::Mono,
            ),
        ],
        outputs: vec![
            bus(
                "audio",
                "Main",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            ),
            bus(
                "bus:1",
                "Aux",
                ProviderBusKind::Auxiliary,
                1,
                ProviderChannelFormat::Discrete,
            ),
        ],
    };
    let execution = GraphCompiler::default()
        .compile(
            &GraphDocument::new(1, vec![dormant(Some(ports))], vec![]),
            context(),
        )
        .unwrap();
    let node = &execution.nodes()[0];
    assert_eq!(
        node.inputs()
            .iter()
            .map(|port| (port.id().as_str(), port.channels()))
            .collect::<Vec<_>>(),
        [("audio", 2), ("bus:1", 1)]
    );
    assert_eq!(
        node.outputs()
            .iter()
            .map(|port| (port.id().as_str(), port.channels()))
            .collect::<Vec<_>>(),
        [("audio", 2), ("bus:1", 1)]
    );
}

#[test]
fn provider_port_validation_rejects_aggregate_bounds_and_legacy_shape() {
    let too_many_channels = ProviderPorts {
        layout_id: "layout".into(),
        inputs: vec![
            bus(
                "audio",
                "Main",
                ProviderBusKind::Main,
                40,
                ProviderChannelFormat::Discrete,
            ),
            bus(
                "bus:1",
                "Aux",
                ProviderBusKind::Auxiliary,
                25,
                ProviderChannelFormat::Discrete,
            ),
        ],
        outputs: vec![bus(
            "audio",
            "Main",
            ProviderBusKind::Main,
            2,
            ProviderChannelFormat::Stereo,
        )],
    };
    assert!(matches!(
        too_many_channels.validate(),
        Err(ProviderPortsError::TooManyTotalChannels {
            direction: "input",
            ..
        })
    ));
    assert!(
        serde_json::from_value::<ProviderPorts>(serde_json::json!({
            "inputs": 2,
            "outputs": 2
        }))
        .is_err()
    );
}

#[test]
fn provider_port_validation_rejects_duplicate_ids_and_format_mismatch() {
    let mut ports = ProviderPorts {
        layout_id: "layout".into(),
        inputs: vec![
            bus(
                "audio",
                "Main",
                ProviderBusKind::Main,
                2,
                ProviderChannelFormat::Stereo,
            ),
            bus(
                "audio",
                "Side",
                ProviderBusKind::Sidechain,
                1,
                ProviderChannelFormat::Mono,
            ),
        ],
        outputs: vec![bus(
            "audio",
            "Main",
            ProviderBusKind::Main,
            2,
            ProviderChannelFormat::Stereo,
        )],
    };
    assert!(matches!(
        ports.validate(),
        Err(ProviderPortsError::DuplicateBusId {
            direction: "input",
            ..
        })
    ));
    ports.inputs[1].id = PortId::from("bus:1");
    ports.inputs[1].channel_format = ProviderChannelFormat::Stereo;
    assert!(matches!(
        ports.validate(),
        Err(ProviderPortsError::ChannelFormatMismatch { .. })
    ));
}

#[test]
fn provider_port_identity_bounds_and_empty_display_names_are_enforced() {
    let mut ports = ProviderPorts::single_bus("layout", 2, 2);
    ports.inputs[0].name.clear();
    assert!(ports.validate().is_ok());

    ports.layout_id = "x".repeat(crate::MAX_PROVIDER_LAYOUT_ID_BYTES + 1);
    assert!(matches!(
        ports.validate(),
        Err(ProviderPortsError::IdentityTooLong {
            identity: "layout id",
            ..
        })
    ));
    ports.layout_id = "layout".into();
    ports.inputs[0].id = PortId::from("x".repeat(crate::MAX_PROVIDER_BUS_ID_BYTES + 1).as_str());
    assert!(matches!(
        ports.validate(),
        Err(ProviderPortsError::IdentityTooLong {
            identity: "bus id",
            ..
        })
    ));
}

#[test]
fn provider_port_validation_rejects_more_than_sixty_four_buses() {
    let ports = ProviderPorts {
        layout_id: "layout".into(),
        inputs: (0..=crate::MAX_PROVIDER_BUSES_PER_DIRECTION)
            .map(|index| {
                bus(
                    &format!("bus:{index}"),
                    "",
                    ProviderBusKind::Auxiliary,
                    1,
                    ProviderChannelFormat::Mono,
                )
            })
            .collect(),
        outputs: vec![bus(
            "audio",
            "Main",
            ProviderBusKind::Main,
            2,
            ProviderChannelFormat::Stereo,
        )],
    };
    assert!(matches!(
        ports.validate(),
        Err(ProviderPortsError::TooManyBuses {
            direction: "input",
            ..
        })
    ));
}

#[test]
fn dormant_provider_does_not_hide_configuration_errors_from_resolver() {
    struct Invalid;
    impl NodeResolver for Invalid {
        fn resolve(
            &self,
            _: &NodeSpec,
            _: &CompileContext,
        ) -> Result<ResolvedNode, NodeResolutionError> {
            Err(NodeResolutionError::InvalidConfiguration {
                reason: "invalid fixture state".into(),
            })
        }
    }
    let document = GraphDocument::new(
        1,
        vec![dormant(Some(ProviderPorts::single_bus(
            "test-layout",
            2,
            2,
        )))],
        vec![],
    );
    let error = GraphCompiler::new(Invalid)
        .compile(&document, context())
        .unwrap_err();
    assert!(
        matches!(error.issues(), [GraphIssue::NodeResolutionFailed { reason, .. }]
        if reason.contains("invalid fixture state"))
    );
}
