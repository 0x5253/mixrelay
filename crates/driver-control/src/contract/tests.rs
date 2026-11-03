use std::collections::BTreeMap;

use mixrelay_audio_io::{
    ChannelLayout, NativeDeviceFormats, NativeSampleFormat, NativeStreamFormat,
    NativeStreamFormats, StreamSpec, VirtualEndpointCapabilities, VirtualEndpointDeviceSettings,
    VirtualEndpointId, VirtualEndpointSettings, VirtualEndpointSpec,
};
use mixrelay_test_support::{every_variant, reply_fixture::refresh_reply_fixture};
use serde_json::json;

use super::{DriverSnapshot, DriverState};

/// A stale fixture is rewritten and this fails; review the diff and commit it
/// (`pnpm fixtures:replies` reruns every fixture owner).
#[test]
fn driver_snapshot_reply_fixture_matches_the_serialized_contract() {
    let snapshot = DriverSnapshot {
        protocol_version: 1,
        revision: 5,
        state: DriverState::Ready,
        installed_version: Some("1.2.10".to_owned()),
        owner_process_id: Some(412),
        settings: VirtualEndpointSettings {
            devices: vec![device()],
            ..VirtualEndpointSettings::configurable(&VirtualEndpointCapabilities {
                max_render_channels: 2,
                max_capture_channels: 2,
                supported_sample_rates: vec![44_100, 48_000],
                supports_duplex: true,
                supports_dynamic_creation: true,
            })
        },
        persistence_error: Some("disk full".to_owned()),
    };
    let states = BTreeMap::from([(
        "state",
        json!(every_variant!(DriverState {
            Ready,
            NotInstalled,
            InstalledNotLoaded,
            Unavailable,
        })),
    )]);
    let snapshot_stale = refresh_reply_fixture("driver-snapshot", &snapshot).unwrap();
    let states_stale = refresh_reply_fixture("driver-snapshot.variants", &states).unwrap();
    assert!(
        !snapshot_stale && !states_stale,
        "driver-snapshot reply fixtures were stale and have been rewritten; review and commit them"
    );
}

fn device() -> VirtualEndpointDeviceSettings {
    let format = NativeStreamFormat {
        sample_rate: 48_000,
        channels: 2,
        sample_format: NativeSampleFormat::Float32,
    };
    VirtualEndpointDeviceSettings {
        spec: VirtualEndpointSpec {
            id: VirtualEndpointId::new("relay-1".to_owned()),
            display_name: "MixRelay 1".to_owned(),
            render: None,
            capture: Some(StreamSpec {
                channels: 2,
                channel_layout: ChannelLayout::Stereo,
            }),
            supported_sample_rates: vec![48_000],
            default_sample_rate: 48_000,
        },
        native_device_ids: vec!["MixRelayDevice:relay-1".to_owned()],
        observed_sample_rate: None,
        supported_sample_rates: vec![48_000],
        can_set_sample_rate: false,
        native_formats: NativeDeviceFormats {
            render: NativeStreamFormats::default(),
            capture: NativeStreamFormats {
                current: None,
                supported: vec![format],
            },
        },
    }
}
