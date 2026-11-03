use super::*;

fn packet() -> ControlTelemetry {
    ControlTelemetry {
        engine_incarnation: "engine-a".into(),
        sequence: 1,
        meters: MeterSnapshot {
            protocol_version: 1,
            revision: 3,
            nodes: vec![
                NodeMeterSnapshot {
                    node_id: "n".into(),
                    port_id: "out".into(),
                    peaks: vec![0.5, 0.25],
                    dynamics: Some(DynamicsMeterSnapshot {
                        input_peaks: vec![0.75],
                        gain_reduction_db: 6.0,
                        threshold_db: -12.0,
                    }),
                },
                NodeMeterSnapshot {
                    node_id: "plain".into(),
                    port_id: "in".into(),
                    peaks: vec![1.0],
                    dynamics: None,
                },
            ],
        },
        state: Some(ControlStateSnapshot {
            generation: 1,
            status: AppStatus::default(),
            graph: GraphRuntimeStatus {
                pending_reason: None,
                protocol_version: 1,
                active_revision: 3,
                submitted_revision: 3,
            },
            controls: None,
            editors: vec![],
        }),
    }
}

#[test]
fn packed_json_and_aligned_raw_frame_preserve_exact_float_bytes_and_bindings() {
    let packet = packet();
    let value = serde_json::to_value(&packet).unwrap();
    assert!(value["meters"].get("nodes").is_none());
    assert_eq!(
        serde_json::from_value::<ControlTelemetry>(value).unwrap(),
        packet
    );
    let frame = TelemetryEncoder::default().raw(&packet);
    let length = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    let metadata: serde_json::Value = serde_json::from_slice(&frame[4..length + 4]).unwrap();
    assert_eq!(metadata["state"]["generation"], 1);
    assert_eq!(metadata["meters"]["samples"], "");
    let offset = (length + 7) & !3;
    let expected: Vec<u8> = [0.5_f32, 0.25, 0.75, 6.0, -12.0, 1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    assert_eq!(frame[offset..], expected);
}

#[test]
fn a_stream_sends_bindings_once_until_they_change_and_decodes_them_from_its_cache() {
    let mut encoder = TelemetryEncoder::default();
    let mut decoder = TelemetryDecoder::default();
    let first = packet();
    let mut steady = packet();
    steady.sequence = 2;
    steady.state = None;
    steady.meters.nodes[1].peaks[0] = 0.125;
    let mut changed = steady.clone();
    changed.sequence = 3;
    changed.meters.nodes.pop();
    let wire: Vec<_> = [&first, &steady, &changed]
        .into_iter()
        .map(|packet| encoder.json(packet))
        .collect();
    assert!(wire[0]["meters"].get("bindings").is_some());
    assert!(wire[1]["meters"].get("bindings").is_none());
    assert!(wire[1].get("state").is_none());
    assert_eq!(wire[2]["meters"]["bindings"].as_array().unwrap().len(), 1);
    for (value, expected) in wire.into_iter().zip([first, steady, changed]) {
        assert_eq!(decoder.json(value).unwrap(), expected);
    }
    // A raw frame carries the same metadata; a fresh encoder starts with bindings.
    let frame = TelemetryEncoder::default().raw(&packet());
    let length = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    let metadata: serde_json::Value = serde_json::from_slice(&frame[4..length + 4]).unwrap();
    assert!(metadata["meters"].get("bindings").is_some());
    // A stream joined without bindings cannot be decoded, nor can malformed JSON.
    let mut late = TelemetryEncoder::default();
    let _ = late.json(&packet());
    let omitted = late.json(&packet());
    assert!(matches!(
        TelemetryDecoder::default().json(omitted.clone()),
        Err(TelemetryDecodeError::MissingBindings)
    ));
    assert!(serde_json::from_value::<ControlTelemetry>(omitted).is_err());
    assert!(matches!(
        TelemetryDecoder::default().json(serde_json::json!({})),
        Err(TelemetryDecodeError::Json(_))
    ));
    let mut invalid = TelemetryEncoder::default().json(&packet());
    invalid["meters"]["samples"] = "?".into();
    assert!(matches!(
        TelemetryDecoder::default().json(invalid),
        Err(TelemetryDecodeError::Base64(_))
    ));
}

#[test]
fn mailbox_retains_latest_state_through_meter_only_windows_and_resets_on_engine_change() {
    let mut older = packet();
    let mut newer = packet();
    newer.sequence = 2;
    newer.state = None;
    newer.meters.nodes[0].peaks[0] = 0.1;
    older.absorb(newer.clone());
    assert_eq!(older.state.as_ref().unwrap().generation, 1);
    assert_eq!(older.meters.nodes[0].peaks[0].to_bits(), 0.5_f32.to_bits());
    newer.state = packet().state;
    newer.state.as_mut().unwrap().generation = 9;
    older.absorb(newer.clone());
    assert_eq!(older.state.as_ref().unwrap().generation, 9);
    newer.engine_incarnation = "replacement".into();
    newer.state = None;
    older.absorb(newer);
    assert!(older.state.is_none());
    assert_eq!(older.meters.nodes[0].peaks[0].to_bits(), 0.1_f32.to_bits());
}

#[test]
fn malformed_base64_lengths_and_overflowing_bindings_are_rejected() {
    let value = serde_json::to_value(packet()).unwrap();
    for samples in ["?", "AA==", ""] {
        let mut invalid = value.clone();
        invalid["meters"]["samples"] = samples.into();
        assert!(serde_json::from_value::<ControlTelemetry>(invalid).is_err());
    }
    for (channels, input) in [
        (usize::MAX, Some(0)),
        (usize::MAX, None),
        (0, Some(usize::MAX)),
        (usize::MAX / 2, None),
    ] {
        let mut invalid = value.clone();
        invalid["meters"]["bindings"][0]["channels"] = channels.into();
        invalid["meters"]["bindings"][0]["inputChannels"] = serde_json::json!(input);
        assert!(serde_json::from_value::<ControlTelemetry>(invalid).is_err());
    }
    let mut stateless = value.clone();
    stateless.as_object_mut().unwrap().remove("state");
    assert!(
        serde_json::from_value::<ControlTelemetry>(stateless)
            .unwrap()
            .state
            .is_none()
    );
    let unpacked = serde_json::json!({"engineIncarnation":"engine-a", "sequence":1,
        "meters":{"protocolVersion":1,"revision":0,"nodes":[]}});
    assert!(serde_json::from_value::<ControlTelemetry>(unpacked).is_err());
    assert!(serde_json::from_value::<ControlTelemetry>(serde_json::json!({})).is_err());
}

#[test]
fn graph_request_conversion_retains_the_document_and_defaults_explicit_intent() {
    let document = mixrelay_audio_graph::GraphDocument::new(7, vec![], vec![]);
    let request = crate::GraphApplyRequest::from(document.clone());
    assert_eq!(request.document, document);
    assert!(request.processor_parameter_changes.is_empty());
    let mut invalid = serde_json::to_value(packet()).unwrap();
    invalid["meters"] = serde_json::json!({});
    assert!(serde_json::from_value::<ControlTelemetry>(invalid).is_err());
}
