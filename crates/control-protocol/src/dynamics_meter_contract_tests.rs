use super::{DynamicsMeterSnapshot, NodeMeterSnapshot};

#[test]
fn meters_without_dynamics_omit_them_and_actual_dynamics_roundtrip_without_peak_inference() {
    let mut meter: NodeMeterSnapshot = serde_json::from_value(serde_json::json!({
        "nodeId":"compressor", "portId":"audio", "peaks":[1.5,0.75],
    }))
    .unwrap();
    assert!(meter.dynamics.is_none());
    assert!(
        serde_json::to_value(&meter)
            .unwrap()
            .get("dynamics")
            .is_none()
    );
    meter.dynamics = Some(DynamicsMeterSnapshot {
        input_peaks: vec![1.0, 0.5],
        gain_reduction_db: 13.5,
        threshold_db: -18.0,
    });
    let encoded = serde_json::to_value(&meter).unwrap();
    assert_eq!(encoded["dynamics"]["gainReductionDb"], 13.5);
    assert_eq!(encoded["dynamics"]["thresholdDb"], -18.0);
    assert_eq!(
        serde_json::from_value::<NodeMeterSnapshot>(encoded).unwrap(),
        meter
    );
}
