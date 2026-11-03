use super::*;

#[test]
fn absent_report_is_zero_but_malformed_metadata_is_unknown() {
    assert_eq!(latency_snapshot(LatencyPort::NotReported, None), Some(0));
    assert_eq!(latency_snapshot(LatencyPort::Unknown, Some(0.0)), None);
}

#[test]
fn latency_port_scalar_requires_a_finite_nonnegative_integer_frame_count() {
    assert_eq!(latency_frames_from_lv2_value(0.0), Some(0));
    assert_eq!(latency_frames_from_lv2_value(128.0), Some(128));
    assert_eq!(latency_frames_from_lv2_value(f32::NAN), None);
    assert_eq!(latency_frames_from_lv2_value(f32::INFINITY), None);
    assert_eq!(latency_frames_from_lv2_value(-1.0), None);
    assert_eq!(latency_frames_from_lv2_value(1.5), None);
    assert_eq!(
        latency_frames_from_lv2_value(4_294_967_040.0),
        Some(4_294_967_040)
    );
    assert_eq!(latency_frames_from_lv2_value(4_294_967_296.0), None);
}

#[test]
fn connected_latency_port_snapshot_can_change_between_runs() {
    let port = LatencyPort::OutputControl { offset: 2 };
    assert_eq!(latency_snapshot(port, Some(24.0)), Some(24));
    assert_eq!(latency_snapshot(port, Some(72.0)), Some(72));
    assert_eq!(latency_snapshot(port, None), None);
}
