use super::*;
use crate::{format::EndpointObservation, runtime::render_format, test_support::FakeDriver};
use mixrelay_audio_io::{
    ChannelLayout, NativeDeviceFormats, NativeSampleFormat, NativeStreamFormats, StreamSpec,
};
use std::sync::Arc;

fn format(sample_rate: u32, sample_format: NativeSampleFormat) -> NativeStreamFormat {
    NativeStreamFormat {
        sample_rate,
        channels: 2,
        sample_format,
    }
}

fn spec(id: &str) -> VirtualEndpointSpec {
    let stereo = StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    };
    VirtualEndpointSpec {
        id: id.into(),
        display_name: format!("{id} display"),
        render: Some(stereo),
        capture: Some(stereo),
        supported_sample_rates: vec![44_100, 48_000],
        default_sample_rate: 48_000,
    }
}

/// Render is Float32 and capture is Signed16; both support 44.1 and 48 kHz.
fn device(id: &str) -> VirtualEndpointDeviceSettings {
    let direction = |sample_format| NativeStreamFormats {
        current: Some(format(48_000, sample_format)),
        supported: vec![format(44_100, sample_format), format(48_000, sample_format)],
    };
    VirtualEndpointDeviceSettings {
        spec: spec(id),
        native_device_ids: Vec::new(),
        observed_sample_rate: Some(48_000),
        supported_sample_rates: vec![44_100, 48_000],
        can_set_sample_rate: true,
        native_formats: NativeDeviceFormats {
            render: direction(NativeSampleFormat::Float32),
            capture: direction(NativeSampleFormat::Signed16),
        },
    }
}

fn snapshot(devices: Vec<VirtualEndpointDeviceSettings>) -> DriverSnapshot {
    DriverSnapshot {
        protocol_version: 1,
        revision: 7,
        state: DriverState::Ready,
        installed_version: None,
        owner_process_id: None,
        settings: VirtualEndpointSettings {
            can_create: true,
            can_rename: true,
            render_channels: vec![2],
            capture_channels: vec![2],
            creation_template: None,
            devices,
        },
        persistence_error: None,
    }
}

fn backend(driver: &Arc<FakeDriver>) -> Arc<LinuxVirtualEndpointBackend> {
    LinuxVirtualEndpointBackend::new(
        Arc::clone(driver) as _,
        render_format(),
        Arc::new(EndpointObservation::default()),
        mixrelay_time::StdClock::new(),
    )
}

#[test]
fn rate_change_is_one_revision_checked_mutation_keeping_pcm_representation() {
    let driver = FakeDriver::new(snapshot(vec![device("mix")]));
    backend(&driver)
        .set_sample_rate(&"mix".into(), 44_100)
        .unwrap();
    assert_eq!(
        driver.mutations(),
        [DriverCommand::SetFormat {
            expected_revision: 7,
            endpoint_id: "mix".into(),
            direction: NativeDirection::Render,
            format: format(44_100, NativeSampleFormat::Float32),
        }]
    );
}

#[test]
fn capture_only_device_changes_rate_through_its_capture_direction() {
    let mut capture_only = device("mic");
    capture_only.native_formats.render = NativeStreamFormats::default();
    let driver = FakeDriver::new(snapshot(vec![capture_only]));
    backend(&driver)
        .set_sample_rate(&"mic".into(), 44_100)
        .unwrap();
    assert_eq!(
        driver.mutations(),
        [DriverCommand::SetFormat {
            expected_revision: 7,
            endpoint_id: "mic".into(),
            direction: NativeDirection::Capture,
            format: format(44_100, NativeSampleFormat::Signed16),
        }]
    );
}

#[test]
fn unsupported_or_unobserved_rates_never_reach_the_driver() {
    let mut capture_lacks_rate = device("mix");
    capture_lacks_rate
        .native_formats
        .capture
        .supported
        .retain(|format| format.sample_rate == 48_000);
    let mut unobserved = device("idle");
    unobserved.native_formats = NativeDeviceFormats::default();
    let driver = FakeDriver::new(snapshot(vec![capture_lacks_rate, unobserved]));
    let backend = backend(&driver);
    for (id, rate) in [
        ("mix", 44_100),
        ("mix", 96_000),
        ("idle", 44_100),
        ("gone", 44_100),
    ] {
        assert!(
            backend.set_sample_rate(&id.into(), rate).is_err(),
            "{id} {rate}"
        );
    }
    assert!(driver.mutations().is_empty());
}

#[test]
fn graph_devices_resolve_to_driver_specs_only_while_the_driver_is_ready() {
    let mut stale = spec("mix");
    stale.display_name = "graph copy".into();
    let driver = FakeDriver::new(snapshot(vec![device("mix")]));
    let backend = backend(&driver);
    assert_eq!(backend.resolve_graph_devices(&[]).unwrap(), []);
    assert_eq!(
        backend.resolve_graph_devices(&[stale.clone()]).unwrap(),
        [spec("mix")]
    );
    assert!(
        backend
            .resolve_graph_devices(&[stale.clone(), spec("gone")])
            .is_err()
    );
    assert_eq!(backend.sample_rate(&"mix".into()).unwrap(), 48_000);

    driver.set_state(DriverState::InstalledNotLoaded);
    assert!(backend.settings().is_none());
    assert!(backend.sample_rate(&"mix".into()).is_err());
    assert!(backend.prepare_graph_attachments(&[stale]).is_err());
    assert!(backend.prepare_graph_attachments(&[]).is_ok());
}
