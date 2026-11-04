use super::*;
use mixrelay_audio_io::{
    NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat, NativeStreamFormats,
    VirtualEndpointDeviceSettings, VirtualEndpointSettings,
    session::{AudioBackendKind, AudioSessionCapabilities, ClockAuthority, SessionFormat},
};

struct ConfigurableBackend(
    VirtualEndpointCapabilities,
    Vec<VirtualEndpointDeviceSettings>,
);

struct CatalogReadbackBackend {
    base: Arc<dyn VirtualEndpointBackend>,
    owner: Mutex<std::sync::Weak<EngineGraphController>>,
}

impl VirtualEndpointBackend for CatalogReadbackBackend {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        self.base.capabilities()
    }

    fn settings(&self) -> Option<VirtualEndpointSettings> {
        if let Some(owner) = self.owner.lock().upgrade() {
            assert!(
                owner.catalog.is_unlocked(),
                "Driver read must not hold catalog"
            );
        }
        let mut settings = self.base.settings()?;
        settings.devices.clear();
        let document = endpoint_document(1, "Native name");
        let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config else {
            panic!("endpoint fixture")
        };
        let format = NativeStreamFormat {
            sample_rate: 48_000,
            channels: 64,
            sample_format: NativeSampleFormat::Float32,
        };
        settings.devices.push(VirtualEndpointDeviceSettings {
            spec: endpoint.spec.clone(),
            native_device_ids: vec!["native-readback-id".into()],
            observed_sample_rate: Some(48_000),
            supported_sample_rates: vec![48_000],
            can_set_sample_rate: true,
            native_formats: NativeDeviceFormats {
                render: NativeStreamFormats {
                    current: Some(format),
                    supported: vec![format],
                },
                capture: NativeStreamFormats::default(),
            },
        });
        Some(settings)
    }

    fn sample_rate(&self, _: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        let owner = self.owner.lock().upgrade().unwrap();
        assert!(
            owner.catalog.is_unlocked(),
            "native read must not hold catalog"
        );
        Ok(48_000)
    }

    fn prepare_graph_attachments(
        &self,
        specs: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.base.prepare_graph_attachments(specs)
    }
}

#[test]
fn driver_readback_does_not_hold_catalog_and_overrides_copied_graph_specs() {
    let mut engine = controller(endpoint_document(1, "Readback"));
    let backend = Arc::new(CatalogReadbackBackend {
        base: engine.endpoint_backend.clone(),
        owner: Mutex::new(std::sync::Weak::new()),
    });
    engine.endpoint_backend = backend.clone();
    let engine = Arc::new(engine);
    *backend.owner.lock() = Arc::downgrade(&engine);
    let observed = engine.endpoint_settings().unwrap();
    assert_eq!(observed.devices.len(), 1);
    assert_eq!(observed.devices[0].spec.display_name, "Native name");
    assert_eq!(observed.devices[0].observed_sample_rate, Some(48_000));
    assert_eq!(
        observed.devices[0].native_device_ids,
        ["native-readback-id"]
    );
    let formats = &observed.devices[0].native_formats;
    assert_eq!(formats.render.supported.len(), 1);
    assert_eq!(formats.render.current, Some(formats.render.supported[0]));
    assert_eq!(formats.render.supported[0].channels, 64);
    assert_eq!(
        formats.render.supported[0].sample_format,
        NativeSampleFormat::Float32
    );
    assert_eq!(formats.capture, NativeStreamFormats::default());
}

impl Default for ConfigurableBackend {
    fn default() -> Self {
        Self(
            VirtualEndpointCapabilities {
                max_render_channels: 64,
                max_capture_channels: 64,
                supported_sample_rates: mixrelay_audio_io::formats::SAMPLE_RATES.to_vec(),
                supports_duplex: true,
                supports_dynamic_creation: true,
            },
            Vec::new(),
        )
    }
}

impl VirtualEndpointBackend for ConfigurableBackend {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        &self.0
    }
    fn settings(&self) -> Option<VirtualEndpointSettings> {
        let mut settings = VirtualEndpointSettings::configurable(self.capabilities());
        settings.devices.clone_from(&self.1);
        Some(settings)
    }
    fn prepare_graph_attachments(
        &self,
        specs: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.resolve_graph_devices(specs).map(|_| ())
    }

    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        desired
            .iter()
            .map(|reference| {
                self.1
                    .iter()
                    .find(|device| device.spec.id == reference.id)
                    .map(|device| device.spec.clone())
                    .ok_or_else(|| HostAudioBackendError::BackendFailure {
                        message: "Driver device is missing".into(),
                    })
            })
            .collect()
    }
}

pub(super) fn controller(document: GraphDocument) -> EngineGraphController {
    let inventory = document.clone();
    controller_with_inventory(document, &inventory)
}

/// Seeds an independent Driver inventory before attaching any graph references.
pub(super) fn controller_with_inventory(
    document: GraphDocument,
    inventory: &GraphDocument,
) -> EngineGraphController {
    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let devices = inventory
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeConfig::VirtualEndpoint(config) = &node.config else {
                return None;
            };
            Some(VirtualEndpointDeviceSettings {
                spec: config.spec.clone(),
                native_device_ids: Vec::new(),
                observed_sample_rate: None,
                supported_sample_rates: config.supported_sample_rates.clone(),
                can_set_sample_rate: false,
                native_formats: NativeDeviceFormats::default(),
            })
        })
        .collect();
    let backend = ConfigurableBackend {
        1: devices,
        ..ConfigurableBackend::default()
    };
    EngineGraphController::new_with_initial_document(
        Arc::new(ProcessorExchange::new(document.revision)),
        EngineGraphControllerConfig {
            clock: std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            format,
            initial_bindings: vec![],
            endpoint_backend: Arc::new(backend),
            required_endpoint_node_id: None,
            graph_resources: Arc::new(NoopGraphResourceBackend),
            runtime_status: Arc::new(session::AdapterRuntimeStatus::new(
                session::AdapterProfile {
                    initial_format: SessionFormat {
                        sample_rate: 48_000,
                        block_frames: 128,
                    },
                    capabilities: AudioSessionCapabilities {
                        backend: AudioBackendKind::ContractOnly,
                        supported_sample_rates: vec![44_100, 48_000],
                        supported_block_frames: vec![128],
                        can_reconfigure_live: false,
                        hardware_input: false,
                        hardware_output: false,
                        virtual_endpoint: true,
                        application_capture: false,
                        sample_rate_conversion: false,
                        drift_correction: false,
                    },
                    application_capture_rates: vec![],
                    clock_authority: ClockAuthority::EngineTimer,
                    transport_latency_frames: None,
                    requires_observed_device_rate: false,
                },
                std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            )),

            ..crate::test_support::configuration(format)
        },
        document,
    )
    // Busy rejections are asserted, not timed; a short admission keeps them fast.
    .with_control_admission(std::time::Duration::from_millis(10))
}

pub(super) fn settings(controller: &EngineGraphController) -> VirtualEndpointSettings {
    controller
        .status()
        .audio_session
        .unwrap()
        .virtual_endpoint_settings
        .unwrap()
}

pub(super) fn endpoint_document(revision: u64, name: &str) -> GraphDocument {
    GraphDocument::new(
        revision,
        vec![NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("graph-device"),
            label: Some(name.into()),
            config: NodeConfig::VirtualEndpoint(
                VirtualEndpointSpec {
                    id: VirtualEndpointId::from("7a96b0d1-532a-4893-a3ae-8c805f45de92"),
                    display_name: name.into(),
                    render: Some(StreamSpec {
                        channels: 64,
                        channel_layout: ChannelLayout::Discrete,
                    }),
                    capture: Some(StreamSpec {
                        channels: 16,
                        channel_layout: ChannelLayout::Discrete,
                    }),
                    supported_sample_rates: vec![44_100, 48_000, 96_000],
                    default_sample_rate: 48_000,
                }
                .into(),
            ),
        }],
        vec![],
    )
}

#[test]
fn missing_reference_and_final_reference_removal_preserve_driver_devices() {
    let initial = endpoint_document(1, "SPAT send-return");
    let controller = controller(initial.clone());
    let expected = settings(&controller);
    assert_eq!(expected.devices.len(), 1);
    assert_eq!(expected.devices[0].spec.display_name, "SPAT send-return");
    assert!(expected.creation_template.is_some());
    assert_eq!(
        expected.devices[0].native_formats,
        NativeDeviceFormats::default()
    );
    let mut rejected = initial;
    rejected.revision = 2;
    let NodeConfig::VirtualEndpoint(spec) = &mut rejected.nodes[0].config else {
        panic!()
    };
    spec.id = VirtualEndpointId::from("missing-driver-device");
    assert!(controller.apply_graph_request(rejected.into()).is_err());
    assert_eq!(settings(&controller), expected);
    controller
        .apply_graph_request(GraphDocument::new(2, vec![], vec![]).into())
        .unwrap();
    assert!(controller.graph().nodes.is_empty());
    assert_eq!(settings(&controller).devices, expected.devices);
    assert_eq!(
        settings(&controller).creation_template,
        expected.creation_template
    );
    assert!(settings(&controller).can_create);
}

#[test]
fn partial_reference_removal_never_changes_shared_device_stream_configuration() {
    let mut initial = endpoint_document(1, "Independent streams");
    let controller = controller(initial.clone());
    let expected = settings(&controller).devices;
    initial.revision = 2;
    let NodeConfig::VirtualEndpoint(config) = &mut initial.nodes[0].config else {
        panic!()
    };
    config.reference.capture = false;
    controller.apply_graph_request(initial.into()).unwrap();
    let processor = crate::controller_test_support::settle(&controller);
    assert_eq!(processor.plan().source_bindings()[0].channels(), 16);
    assert_eq!(
        processor.plan().source_bindings()[0].port_id().as_str(),
        FROM_OS_PORT_ID
    );
    assert!(processor.plan().sink_bindings().is_empty());
    assert_eq!(settings(&controller).devices, expected);
    let graph = controller.graph();
    let NodeConfig::VirtualEndpoint(config) = &graph.nodes[0].config else {
        panic!("endpoint fixture")
    };
    assert!(config.reference.render);
    assert!(!config.reference.capture);
    assert_eq!(config.spec.capture.unwrap().channels, 16);
    assert_eq!(
        settings(&controller).render_channels,
        (1..=64).collect::<Vec<_>>()
    );
}

#[test]
fn empty_startup_offers_new_identity_creation_without_inventing_a_device() {
    let empty = controller(GraphDocument::new(1, vec![], vec![]));
    assert!(settings(&empty).can_create);
    assert!(settings(&empty).creation_template.is_some());
    assert!(settings(&empty).devices.is_empty());
    assert!(empty.graph().nodes.is_empty());
}

#[test]
fn settings_wire_roundtrip_exposes_creation_choices_and_identified_devices() {
    let controller = controller(endpoint_document(1, "SPAT send-return"));
    let status = controller.status();
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(
        json["audioSession"]["virtualEndpointSettings"]["canRename"],
        true
    );
    assert_eq!(
        serde_json::from_value::<AppStatus>(json.clone()).unwrap(),
        status
    );
    let settings = &json["audioSession"]["virtualEndpointSettings"];
    assert!(settings["creationTemplate"].is_object());
    assert_eq!(settings["devices"].as_array().unwrap().len(), 1);
    assert_eq!(
        settings["devices"][0]["spec"]["displayName"],
        "SPAT send-return"
    );
    assert!(settings.get("defaultEndpoint").is_none());
}

#[test]
fn stale_graph_settings_resolve_actual_driver_spec_without_native_mutation() {
    let controller = controller(endpoint_document(1, "Driver name"));
    let mut copied = endpoint_document(2, "Stale graph name");
    let NodeConfig::VirtualEndpoint(config) = &mut copied.nodes[0].config else {
        panic!()
    };
    config.default_sample_rate = 44_100;
    let mut documents = vec![("graph".into(), copied.clone())];
    controller
        .resolve_execution_devices(&mut documents)
        .unwrap();
    let NodeConfig::VirtualEndpoint(resolved) = &documents[0].1.nodes[0].config else {
        panic!()
    };
    assert_eq!(resolved.display_name, "Driver name");
    assert_eq!(resolved.default_sample_rate, 48_000);
    assert_eq!(settings(&controller).devices[0].spec, resolved.spec);
    assert_eq!(copied.nodes[0].label.as_deref(), Some("Stale graph name"));
}

#[test]
fn missing_driver_reference_is_rejected_without_creating_a_device() {
    let controller = controller(GraphDocument::new(1, vec![], vec![]));
    let mut documents = vec![("graph".into(), endpoint_document(2, "Missing"))];
    assert!(
        controller
            .resolve_execution_devices(&mut documents)
            .is_err()
    );
    assert!(settings(&controller).devices.is_empty());
}
