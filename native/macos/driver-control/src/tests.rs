use super::{capabilities, identity, native_spec, observed_spec};
use mixrelay_audio_io::{
    ChannelLayout, NativeSampleFormat, NativeStreamFormat, StreamSpec, VirtualEndpointSpec,
};
use mixrelay_driver_control::DriverDeviceConfig;

fn config() -> DriverDeviceConfig {
    DriverDeviceConfig {
        spec: VirtualEndpointSpec {
            id: "17e59d73-42c1-4f11-a8b4-6c2d913357e0".into(),
            display_name: "Independent Driver".into(),
            render: Some(StreamSpec {
                channels: 2,
                channel_layout: ChannelLayout::Stereo,
            }),
            capture: Some(StreamSpec {
                channels: 4,
                channel_layout: ChannelLayout::Discrete,
            }),
            supported_sample_rates: capabilities().supported_sample_rates,
            default_sample_rate: 48_000,
        },
        render_format: Some(NativeStreamFormat {
            sample_rate: 48_000,
            channels: 2,
            sample_format: NativeSampleFormat::Signed24,
        }),
        capture_format: Some(NativeStreamFormat {
            sample_rate: 48_000,
            channels: 4,
            sample_format: NativeSampleFormat::Float32,
        }),
    }
}

#[test]
fn device_identity_and_duplex_geometry_do_not_depend_on_graph_nodes() {
    let config = config();
    let native = native_spec(&config).unwrap();
    assert_eq!((native.render_channels, native.capture_channels), (2, 4));
    assert_eq!(native.sample_rate, 48_000);
    assert_eq!(
        mixrelay_macos_transport::endpoint_identity::format_endpoint_id(native.id),
        config.spec.id.as_str()
    );
    for invalid in [
        "default",
        "mixrelay.virtual-io",
        "17E59D73-42C1-4F11-A8B4-6C2D913357E0",
    ] {
        assert!(identity(&invalid.into()).is_err());
    }
}

#[test]
fn directional_tuple_must_match_geometry_and_the_one_shared_rate() {
    let mut config = config();
    config.capture_format.as_mut().unwrap().sample_rate = 96_000;
    assert!(native_spec(&config).is_err());
    config.capture_format.as_mut().unwrap().sample_rate = 48_000;
    config.capture_format.as_mut().unwrap().channels = 2;
    assert!(native_spec(&config).is_err());
    config.capture_format = None;
    config.spec.capture = None;
    assert!(native_spec(&config).is_ok());
}

#[test]
fn external_daw_rate_is_returned_without_mutating_the_stored_intent() {
    let configured = config();
    let spec = observed_spec(&configured.spec, Some(96_000), &[44_100, 96_000]);
    assert_eq!(spec.default_sample_rate, 96_000);
    assert_eq!(spec.supported_sample_rates, [44_100, 96_000]);
    assert_eq!(configured.spec.default_sample_rate, 48_000);
    assert_eq!(spec.id, configured.spec.id);
    assert_eq!(spec.display_name, configured.spec.display_name);
    let unavailable = observed_spec(&configured.spec, None, &[]);
    assert!(unavailable.supported_sample_rates.is_empty());
    assert_eq!(unavailable.default_sample_rate, 48_000);
}

#[test]
fn packaged_hal_allows_inventory_and_engine_pcm_mach_services() {
    let plist = include_str!("../../hal-driver/driver/Info.plist");
    let (_, services) = plist
        .split_once("<key>AudioServerPlugIn_MachServices</key>")
        .unwrap();
    let (_, services) = services.split_once("<array>").unwrap();
    let (services, _) = services.split_once("</array>").unwrap();
    let entries: Vec<_> = services
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(
        entries,
        [
            "<string>io.mixrelay.audio.driver</string>",
            "<string>io.mixrelay.audio.engine</string>"
        ]
    );
}

#[test]
fn clearing_latency_for_an_absent_device_forgets_its_cached_value() {
    use mixrelay_driver_control::{
        DriverBackend as _, DriverError, EngineLatencyLease, RuntimeLatency,
    };
    let service = service();
    let endpoint = config().spec.id;
    let id = identity(&endpoint).unwrap();
    service.latencies.lock().insert(
        id,
        mixrelay_macos_transport::control_inventory::InputLatency {
            endpoint_id: id,
            rate: 48_000,
            engine_pid: 10,
            lease: 20,
            revision: 30,
            frames: Some(300),
        },
    );
    let mut driver = super::MacDriver {
        service: std::sync::Arc::clone(&service),
    };
    let mut publication = RuntimeLatency {
        endpoint_id: endpoint,
        rate: 48_000,
        engine_lease: EngineLatencyLease {
            generation: 20,
            incarnation: "engine".into(),
            process_id: 10,
        },
        applied_graph_revision: 30,
        input_latency_frames: None,
    };
    driver.publish_latency(&publication).unwrap();
    assert!(service.latencies.lock().is_empty());
    publication.input_latency_frames = Some(300);
    assert!(matches!(
        driver.publish_latency(&publication),
        Err(DriverError::NotFound(_))
    ));
}

struct NoDevices;
impl super::device_observation::DeviceProperties for NoDevices {
    fn inventory(&self) -> Result<Vec<DriverDeviceConfig>, mixrelay_driver_control::DriverError> {
        Ok(Vec::new())
    }
    fn present(&self, _: [u8; 16]) -> bool {
        false
    }
    fn rate(&self, _: [u8; 16]) -> Option<u32> {
        None
    }
    fn supports(&self, _: [u8; 16], _: u32) -> bool {
        false
    }
    fn formats(&self, _: [u8; 16]) -> mixrelay_audio_io::NativeDeviceFormats {
        mixrelay_audio_io::NativeDeviceFormats::default()
    }
    fn set_rate(&self, _: [u8; 16], _: u32) -> bool {
        false
    }
    fn set_format(&self, _: [u8; 16], _: bool, _: NativeStreamFormat) -> bool {
        false
    }
}
struct Installed;
impl super::device_observation::HalInstallation for Installed {
    fn installed(&self) -> bool {
        true
    }
}
pub(crate) fn service() -> std::sync::Arc<super::Service> {
    super::Service::unpublished(
        mixrelay_macos_transport::control_inventory::ProducerPolicy::default(),
        mixrelay_time::ManualClock::default(),
        NoDevices,
        Installed,
    )
}

#[test]
fn empty_hal_bootstrap_keeps_inventory_unpublished_using_only_injected_observations() {
    let service = service();
    let mut driver = super::MacDriver {
        service: std::sync::Arc::clone(&service),
    };
    driver.bootstrap().unwrap();
    assert!(service.published.lock().is_none());
    assert_eq!(*service.generation.lock(), 0);
    assert!(service.configuration.lock().is_empty());
    assert_eq!(
        mixrelay_driver_control::DriverBackend::state(&driver),
        mixrelay_driver_control::DriverState::InstalledNotLoaded
    );
}
