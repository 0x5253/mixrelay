use super::{DynamicsMeterSnapshot, MeterSnapshot, MeterTelemetry, NodeMeterSnapshot};

fn node(id: &str, peaks: &[f32], reduction: Option<f32>) -> NodeMeterSnapshot {
    NodeMeterSnapshot {
        node_id: id.to_owned(),
        port_id: "out".to_owned(),
        peaks: peaks.to_vec(),
        dynamics: reduction.map(|gain_reduction_db| DynamicsMeterSnapshot {
            input_peaks: peaks.to_vec(),
            gain_reduction_db,
            threshold_db: -gain_reduction_db,
        }),
    }
}

fn window(
    engine: &str,
    revision: u64,
    sequence: u64,
    nodes: Vec<NodeMeterSnapshot>,
) -> MeterTelemetry {
    MeterTelemetry {
        engine_incarnation: engine.to_owned(),
        sequence,
        meters: MeterSnapshot {
            protocol_version: 1,
            revision,
            nodes,
        },
    }
}

#[test]
fn a_slow_subscriber_keeps_the_interval_maximum_of_the_latest_bindings() {
    let mut pending = window(
        "a",
        3,
        1,
        vec![
            node("loud", &[0.9, 0.1], Some(6.0)),
            node("gone", &[1.0], None),
            node("resized", &[0.8], None),
            node("plain", &[0.7], Some(1.0)),
        ],
    );
    pending.absorb(window(
        "a",
        3,
        2,
        vec![
            node("loud", &[0.2, 0.5], Some(2.0)),
            node("new", &[0.3], None),
            node("resized", &[0.1, 0.1], None),
            node("plain", &[0.2], None),
        ],
    ));
    assert_eq!(
        pending,
        window(
            "a",
            3,
            2,
            vec![
                NodeMeterSnapshot {
                    dynamics: Some(DynamicsMeterSnapshot {
                        input_peaks: vec![0.9, 0.5],
                        gain_reduction_db: 6.0,
                        threshold_db: -2.0,
                    }),
                    ..node("loud", &[0.9, 0.5], None)
                },
                node("new", &[0.3], None),
                node("resized", &[0.1, 0.1], None),
                node("plain", &[0.7], None),
            ],
        )
    );
}

#[test]
fn another_engine_or_plan_replaces_the_pending_window() {
    let newer = window("b", 3, 1, vec![node("x", &[0.1], None)]);
    let mut pending = window("a", 3, 9, vec![node("x", &[1.0], None)]);
    pending.absorb(newer.clone());
    assert_eq!(pending, newer);
    let newer = window("b", 4, 2, vec![node("x", &[0.1], None)]);
    pending.absorb(newer.clone());
    assert_eq!(pending, newer);
    let mut newer = window("b", 4, 3, vec![node("x", &[0.1], None)]);
    newer.meters.protocol_version = 2;
    pending.absorb(newer.clone());
    assert_eq!(pending, newer);
}
