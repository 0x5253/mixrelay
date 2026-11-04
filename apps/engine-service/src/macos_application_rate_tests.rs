//! Real controller/capability/format-staging path, with native rate negotiation
//! replaced by an owned in-memory counter. Never opens a tap or changes HAL rate.
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeConfig, NodeSpec, RenderFormat,
};
use mixrelay_audio_io::{
    HostAudioBackendError, VirtualEndpointBackend, VirtualEndpointCapabilities,
    session::{SessionFormat, SessionReconfigureRequest},
};
use mixrelay_control_server::ControlBackend;
use mixrelay_engine_core::{
    ProcessingControlBackend, RuntimeStatusBackend, SessionFormatBackend, default_peak_meter_bank,
    session::AdapterRuntimeStatus,
};
use mixrelay_engine_service::{EngineGraphController, EngineGraphControllerConfig};
use mixrelay_macos_engine::test_support;
use mixrelay_processor_host::ProcessorCatalog;
use mixrelay_render::{BlockProcessor, ProcessorExchange, RenderPlan};
use std::sync::{Arc, Mutex};

struct NativeNegotiationFixture {
    profile: Arc<AdapterRuntimeStatus>,
    accepted: Mutex<Vec<u32>>,
}
impl RuntimeStatusBackend for NativeNegotiationFixture {
    fn status(&self, format: RenderFormat) -> mixrelay_control_protocol::AppStatus {
        self.profile.status(format)
    }
    fn capabilities_for_format(
        &self,
        format: RenderFormat,
    ) -> Option<mixrelay_audio_io::session::AudioSessionCapabilities> {
        self.profile.capabilities_for_format(format)
    }
}

impl ProcessingControlBackend for NativeNegotiationFixture {}

impl SessionFormatBackend for NativeNegotiationFixture {
    fn reconfigure(&self, requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        self.accepted.lock().unwrap().push(requested.sample_rate);
        self.profile.activate_format(requested);
        Ok(())
    }
}

/// A virtual-endpoint backend for graphs that publish no native endpoint.
pub(crate) struct EndpointFixture(VirtualEndpointCapabilities);
impl EndpointFixture {
    pub(crate) fn new() -> Self {
        Self(VirtualEndpointCapabilities {
            max_render_channels: 64,
            max_capture_channels: 64,
            supported_sample_rates: vec![44_100, 48_000],
            supports_duplex: true,
            supports_dynamic_creation: false,
        })
    }
}
impl VirtualEndpointBackend for EndpointFixture {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        &self.0
    }
    fn prepare_graph_attachments(
        &self,
        desired: &[mixrelay_audio_io::VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        assert!(
            desired.is_empty(),
            "fixture never prepares a native endpoint"
        );
        Ok(())
    }
}

fn application_document() -> GraphDocument {
    GraphDocument::new(1, vec![NodeSpec { control: mixrelay_audio_graph::NodeControl::default(), id: "application".into(), label: None,
        config: NodeConfig::ApplicationCapture(serde_json::from_value(serde_json::json!({
            "selector": {"match":"bundle_id", "bundle_id":"org.mixrelay.rate-fixture", "include_descendants":false},
            "stream": {"channels":2,"channelLayout":"stereo"}, "sourcePlayback":"preserve"
        })).unwrap()) }], vec![])
}

#[test]
fn macos_product_profile_covers_common_rates_with_clock_checks_intact() {
    let profile = test_support::adapter_profile();
    assert_eq!(
        profile.application_capture_rates,
        mixrelay_audio_io::formats::SAMPLE_RATES
    );
    assert_eq!(
        profile.capabilities.supported_sample_rates,
        mixrelay_audio_io::formats::SAMPLE_RATES
    );
    assert!(profile.capabilities.sample_rate_conversion);
    assert!(profile.capabilities.drift_correction);
    let status = AdapterRuntimeStatus::new(
        profile,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    )
    .with_graph_format_control();
    status.observe_device_rate(Some(48_000));
    status.observe_endpoint(true);
    // Support must not hide a missing native clock. Only real binding/clock
    // observations can clear the warning, not these capability declarations.
    let unclocked = status.status(RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    });
    assert_eq!(
        unclocked.audio_session.unwrap().issue,
        Some(mixrelay_audio_io::session::SessionIssue::ClockNotSynchronized)
    );
    for rate in [44_100, 48_000, 96_000] {
        let capabilities = status
            .capabilities_for_format(RenderFormat {
                sample_rate: rate,
                block_frames: 128,
            })
            .unwrap();
        assert!(capabilities.application_capture);
    }
}

fn initial_processor(
    document: &GraphDocument,
    format: RenderFormat,
    meters: Arc<mixrelay_render::PeakMeterBank>,
) -> Box<BlockProcessor> {
    let execution = GraphCompiler::default()
        .compile(
            document,
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(
        BlockProcessor::with_peak_meters(
            Arc::new(RenderPlan::prepare(&execution).unwrap()),
            meters,
        )
        .unwrap(),
    )
}

struct ApplicationFixture {
    controller: EngineGraphController,
    runtime: Arc<NativeNegotiationFixture>,
    resources: Arc<mixrelay_macos_engine::MacDevices>,
    exchange: Arc<ProcessorExchange>,
    processor: Box<BlockProcessor>,
}

fn application_fixture() -> ApplicationFixture {
    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let document = application_document();
    let meters = default_peak_meter_bank();
    let processor = initial_processor(&document, format, meters.clone());
    let exchange = Arc::new(ProcessorExchange::new(1));
    let profile = Arc::new(
        AdapterRuntimeStatus::new(
            test_support::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        )
        .with_graph_format_control(),
    );
    let runtime = Arc::new(NativeNegotiationFixture {
        profile: profile.clone(),
        accepted: Mutex::default(),
    });
    let resources = Arc::new(test_support::devices(
        profile,
        48_000,
        256,
        test_support::SimulatedHost::new(),
    ));
    let controller = EngineGraphController::new_with_initial_document(
        exchange.clone(),
        EngineGraphControllerConfig {
            clock: std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            format,
            meters,
            initial_bindings: vec![],
            required_endpoint_node_id: None,
            endpoint_backend: Arc::new(EndpointFixture::new()),
            processor_catalog: ProcessorCatalog::default(),
            graph_resources: resources.clone(),
            runtime_status: runtime.clone(),
        },
        document,
    );
    ApplicationFixture {
        controller,
        runtime,
        resources,
        exchange,
        processor,
    }
}

fn activate_processing_boundary(
    exchange: &ProcessorExchange,
    processor: &mut Box<BlockProcessor>,
    activation: &mut test_support::Activation<'_>,
    active_devices: &mut test_support::ActiveDevices,
) {
    // Match the worker: resource and DSP publication share the same block boundary.
    assert!(
        exchange
            .activate_pending_configured(processor, |_| {
                activation.activate(active_devices);
                Ok::<(), std::convert::Infallible>(())
            })
            .unwrap()
    );
}

#[test]
fn application_graph_reconfigures_every_product_format_through_actual_controller() {
    let ApplicationFixture {
        controller,
        runtime,
        resources,
        exchange,
        mut processor,
    } = application_fixture();
    let mut activation = test_support::activation(&resources);
    let mut active_devices = test_support::ActiveDevices::default();
    let mut revision = 1;
    let mut accepted = Vec::new();
    for (sample_rate, block_frames) in mixrelay_audio_io::formats::SAMPLE_RATES
        .iter()
        .copied()
        .flat_map(|rate| {
            mixrelay_audio_io::formats::BUFFER_SIZES
                .iter()
                .copied()
                .map(move |frames| (rate, frames))
        })
    {
        let response = controller
            .reconfigure_session(SessionReconfigureRequest {
                expected_revision: revision,
                format: SessionFormat {
                    sample_rate,
                    block_frames,
                },
            })
            .unwrap();
        assert_eq!(response.revision, revision + 1);
        assert!(test_support::has_pending(&resources));
        activate_processing_boundary(
            &exchange,
            &mut processor,
            &mut activation,
            &mut active_devices,
        );
        assert!(!test_support::has_pending(&resources));
        assert_eq!(
            processor.plan().format(),
            RenderFormat {
                sample_rate,
                block_frames
            }
        );
        revision += 1;
        accepted.push(sample_rate);
        assert_eq!(controller.graph().nodes[0].id.as_str(), "application");
        assert_eq!(
            controller
                .status()
                .audio_session
                .unwrap()
                .format
                .sample_rate,
            sample_rate
        );
    }
    assert_eq!(*runtime.accepted.lock().unwrap(), accepted);
    assert!(
        controller
            .reconfigure_session(SessionReconfigureRequest {
                expected_revision: revision,
                format: SessionFormat {
                    sample_rate: 384_000,
                    block_frames: 128
                }
            })
            .is_err()
    );
    assert_eq!(
        *runtime.accepted.lock().unwrap(),
        accepted,
        "rate outside the product catalog must not reach negotiation"
    );
}
