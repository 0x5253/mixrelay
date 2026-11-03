use super::*;
use mixrelay_audio_io::{ChannelLayout, StreamSpec, VirtualEndpointId, VirtualEndpointSpec};

pub(crate) fn config(id: &str, channels: u16) -> DriverDeviceConfig {
    DriverDeviceConfig {
        spec: VirtualEndpointSpec {
            id: VirtualEndpointId::from(id),
            display_name: id.into(),
            render: Some(StreamSpec {
                channels,
                channel_layout: ChannelLayout::Discrete,
            }),
            capture: Some(StreamSpec {
                channels,
                channel_layout: ChannelLayout::Discrete,
            }),
            supported_sample_rates: vec![48_000],
            default_sample_rate: 48_000,
        },
        render_format: None,
        capture_format: None,
    }
}

#[test]
fn actual_float_readback_keeps_the_same_plan_but_changed_native_encoding_does_not() {
    let implicit = config("device", 4);
    let plan = DevicePlan::new(&implicit).unwrap();
    let mut observed = implicit.clone();
    [observed.render_format, observed.capture_format] = plan.formats;
    assert!(plan.matches(&observed));
    observed.render_format.as_mut().unwrap().sample_format = NativeSampleFormat::Signed24;
    assert!(!plan.matches(&observed));
}

#[test]
fn direction_tuple_validation_preserves_geometry_rate_and_presence_requirements() {
    let mut request = config("device", 64);
    assert!(DevicePlan::new(&request).is_ok());
    request.capture_format = Some(NativeStreamFormat {
        channels: 64,
        sample_rate: 44_100,
        sample_format: NativeSampleFormat::Signed24,
    });
    assert!(matches!(
        DevicePlan::new(&request),
        Err(PlanError::DirectionTuple)
    ));
    request.capture_format.as_mut().unwrap().sample_rate = 48_000;
    assert!(DevicePlan::new(&request).is_ok());
    request.capture_format.as_mut().unwrap().channels = 32;
    assert!(matches!(
        DevicePlan::new(&request),
        Err(PlanError::DirectionTuple)
    ));
    request.capture_format.as_mut().unwrap().channels = 64;
    request.spec.capture = None;
    assert!(matches!(
        DevicePlan::new(&request),
        Err(PlanError::DirectionTuple)
    ));
    request.spec.default_sample_rate = 0;
    assert!(matches!(
        DevicePlan::new(&request),
        Err(PlanError::Contract(_))
    ));
}
