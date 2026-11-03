//! Native integration fixture for recorder and document ownership tests.
use super::*;
use mixrelay_audio_io::{
    NativeDeviceFormats, VirtualEndpointDeviceSettings, VirtualEndpointSettings,
    session::{AudioBackendKind, AudioSessionCapabilities, ClockAuthority, SessionFormat},
};
struct ConfigurableBackend(
    VirtualEndpointCapabilities,
    Vec<VirtualEndpointDeviceSettings>,
);

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
            meters: default_peak_meter_bank(),
            initial_bindings: vec![],
            endpoint_backend: Arc::new(backend),
            required_endpoint_node_id: None,
            processor_catalog: ProcessorCatalog::default(),
            graph_resources: Arc::new(NoopGraphResourceBackend),
            runtime_status: Arc::new(mixrelay_engine::session::AdapterRuntimeStatus::new(
                mixrelay_engine::session::AdapterProfile {
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
                std::sync::Arc::new(mixrelay_time::StdClock::new()),
            )),
        },
        document,
    )
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
