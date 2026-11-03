#[test]
fn latency_status_distinguishes_unknown_from_zero_and_transport_timing() {
    let mut status = super::AppStatus::default();
    assert!(
        serde_json::to_value(&status)
            .unwrap()
            .get("processorLatencies")
            .is_none()
    );
    for (graph, latency) in [
        ("graph-a", None),
        ("graph-b", Some(0)),
        ("graph-c", Some(u32::MAX)),
    ] {
        status.processor_latencies.push(super::ProcessorLatency {
            graph_id: graph.to_owned(),
            node_id: "effect".to_owned(),
            transport_frames: 128,
            intrinsic_frames: latency,
        });
    }
    let value = serde_json::to_value(&status).unwrap();
    assert!(value.get("processorTimings").is_none());
    let entries = &value["processorLatencies"];
    assert_eq!(entries[0]["graphId"], "graph-a");
    assert_eq!(entries[1]["graphId"], "graph-b");
    assert_eq!(entries[0]["nodeId"], "effect");
    assert_eq!(entries[0]["transportFrames"], 128);
    assert!(entries[0]["intrinsicFrames"].is_null());
    assert_eq!(entries[1]["intrinsicFrames"], 0);
    assert_eq!(entries[2]["intrinsicFrames"], u32::MAX);
    let decoded: super::AppStatus = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.processor_latencies, status.processor_latencies);
}

#[test]
fn processor_timing_status_preserves_graph_identity_and_omits_unmeasured_values() {
    let mut status = super::AppStatus::default();
    let empty = serde_json::to_value(&status).unwrap();
    assert!(empty.get("processorTimings").is_none());
    status.processor_timings.push(super::ProcessorTiming {
        graph_id: "graph-a".to_owned(),
        node_id: "effect-1".to_owned(),
        round_trip_ns: 1_333_333,
    });
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(value["processorTimings"][0]["graphId"], "graph-a");
    assert_eq!(value["processorTimings"][0]["roundTripNs"], 1_333_333);
    let decoded: super::AppStatus = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.processor_timings, status.processor_timings);
}

#[test]
fn failed_processor_status_and_retry_identity_are_exact_and_optional() {
    let mut status = super::AppStatus::default();
    assert!(
        serde_json::to_value(&status)
            .unwrap()
            .get("failedProcessors")
            .is_none()
    );
    status.failed_processors.push(super::FailedProcessor {
        graph_id: "graph-a".into(),
        node_id: "effect".into(),
        instance_id: "18446744073709551615".into(),
    });
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(
        value["failedProcessors"][0]["instanceId"],
        "18446744073709551615"
    );
    let decoded: super::AppStatus = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.failed_processors, status.failed_processors);

    let request = super::ProcessorRetryRequest {
        graph_id: "graph-a".into(),
        node_id: "effect".into(),
        expected_revision: 12,
        expected_catalog_revision: 7,
        expected_instance_id: "18446744073709551615".into(),
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["expectedInstanceId"], "18446744073709551615");
    assert_eq!(
        serde_json::from_value::<super::ProcessorRetryRequest>(value).unwrap(),
        request
    );
}

#[test]
fn graph_layout_request_round_trips_positions_sizes_and_viewport() {
    let layout = serde_json::json!({"expectedLayoutRevision":3,"layout":{
        "positions":{"duplex:from_os":{"x":100.0,"y":-20.0}},
        "sizes":{"duplex:from_os":{"width":320.0,"height":180.0}},
        "viewport":{"x":0.0,"y":0.0,"zoom":1.25}
    }});
    let request: super::GraphLayoutRequest = serde_json::from_value(layout.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), layout);
}

#[test]
fn graph_ui_instance_labels_are_optional_and_round_trip_separately() {
    let unlabeled: super::GraphLayout =
        serde_json::from_value(serde_json::json!({"positions":{}})).unwrap();
    assert!(unlabeled.node_labels.is_empty());
    assert_eq!(
        serde_json::to_value(unlabeled).unwrap(),
        serde_json::json!({"positions":{}})
    );
    let value =
        serde_json::json!({"positions":{}, "nodeLabels":{"app-1":"영상용", "app-2":"회의용"}});
    let layout: super::GraphLayout = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(layout).unwrap(), value);
}

#[test]
fn unavailable_driver_is_distinct_from_policy_block_and_round_trips() {
    let status = super::AppStatus {
        driver_state: super::DriverState::Unavailable,
        ..super::AppStatus::default()
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["driverState"], "unavailable");
    let decoded: super::AppStatus = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.driver_state, super::DriverState::Unavailable);
    assert_ne!(decoded.driver_state, super::DriverState::Blocked);
}
use super::*;

#[test]
fn sample_fault_status_serializes_boundary_counts() {
    let status = AppStatus {
        sample_faults: mixrelay_audio_io::process::SampleFaults {
            source_samples: 2,
            input_samples: 3,
            output_samples: 4,
            blocks: 1,
        },
        ..Default::default()
    };
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(
        value["sampleFaults"],
        serde_json::json!({"sourceSamples":2,"inputSamples":3,"outputSamples":4,"blocks":1})
    );
    assert_eq!(serde_json::from_value::<AppStatus>(value).unwrap(), status);
}

#[test]
fn status_contract_uses_stable_machine_values() {
    let json = serde_json::to_value(AppStatus::default()).expect("status should serialize");

    assert_eq!(json["protocolVersion"], 1);
    assert_eq!(json["driverState"], "not_installed");
    assert!(json["sampleRate"].is_null());
    assert!(json["bufferFrames"].is_null());

    let runtime = serde_json::to_value(GraphRuntimeStatus {
        pending_reason: None,
        protocol_version: PROTOCOL_VERSION,
        active_revision: 4,
        submitted_revision: 5,
    })
    .expect("runtime status should serialize");
    assert_eq!(runtime["activeRevision"], 4);
    assert_eq!(runtime["submittedRevision"], 5);

    let applied = serde_json::to_value(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Pending,
        persistence_error: None,
    })
    .expect("apply response should serialize");
    assert_eq!(applied["state"], "pending");
    assert_eq!(
        serde_json::to_value(GraphApplyState::Active).expect("state should serialize"),
        "active"
    );

    let error = serde_json::to_value(ProtocolError {
        code: ErrorCode::GraphBindingUnsupported,
        parameters: serde_json::json!({ "nodeId": "virtual" }),
    })
    .expect("protocol error should serialize");
    assert_eq!(error["code"], "graph_binding_unsupported");
    assert_eq!(error["parameters"]["nodeId"], "virtual");

    let parameter = serde_json::to_value(ProcessorParameterUpdateResponse {
        protocol_version: PROTOCOL_VERSION,
        node_id: "vst-1".to_owned(),
        parameter_id: 42,
        value: 0.75,
        persistence_error: None,
    })
    .expect("parameter response should serialize");
    assert_eq!(
        serde_json::to_value(ExternalProcessorFormat::Au).expect("format should serialize"),
        "au"
    );
    assert_eq!(
        serde_json::to_value(ExternalProcessorFormat::Auv3).expect("format should serialize"),
        "auv3"
    );
    assert_eq!(
        serde_json::to_value(ExternalProcessorFormat::Clap).expect("format should serialize"),
        "clap"
    );
    assert_eq!(
        serde_json::to_value(ExternalProcessorFormat::Lv2).expect("format should serialize"),
        "lv2"
    );
    assert_eq!(parameter["nodeId"], "vst-1");
    assert_eq!(parameter["parameterId"], 42);
    assert_eq!(parameter["value"], 0.75);

    let editor = serde_json::to_value(ProcessorEditorStatus {
        protocol_version: PROTOCOL_VERSION,
        node_id: "processor-1".to_owned(),
        state: ProcessorEditorState::Failed,
        width: Some(640),
        height: Some(480),
        resizable: true,
        failure: Some(ProcessorEditorFailure::DisplayUnavailable),
    })
    .expect("editor status should serialize");
    assert_eq!(editor["nodeId"], "processor-1");
    assert_eq!(editor["state"], "failed");
    assert_eq!(editor["failure"], "display_unavailable");
    assert_eq!(editor["width"], 640);
}

#[test]
fn parameter_storage_warning_is_optional_and_preserves_the_accepted_value() {
    let mut response: ProcessorParameterUpdateResponse = serde_json::from_value(serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION, "nodeId": "effect", "parameterId": 7, "value": 0.75,
    })).unwrap();
    assert!(response.persistence_error.is_none());
    assert!(
        serde_json::to_value(&response)
            .unwrap()
            .get("persistenceError")
            .is_none()
    );
    response.persistence_error = Some("disk unavailable".into());
    let warned = serde_json::to_value(response).unwrap();
    assert_eq!(
        warned["value"], 0.75,
        "storage warning does not roll back accepted audio"
    );
    assert_eq!(warned["persistenceError"], "disk unavailable");
}

#[test]
fn catalog_diagnostics_are_additive_and_preserve_format_stage_and_candidate_identity() {
    let mut catalog: ExternalProcessorCatalog = serde_json::from_value(serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION, "processors": []
    }))
    .unwrap();
    assert!(catalog.diagnostics.is_empty());
    assert!(
        serde_json::to_value(&catalog)
            .unwrap()
            .get("diagnostics")
            .is_none()
    );
    catalog.diagnostics.push(ExternalProcessorDiagnostic {
        format: ExternalProcessorFormat::Clap,
        stage: ProcessorDiscoveryStage::Probe,
        plugin_id: Some("clap:fixture".to_owned()),
        plugin_name: None,
        message: "child timed out".to_owned(),
    });
    let encoded = serde_json::to_value(&catalog).unwrap();
    assert_eq!(
        encoded["diagnostics"][0],
        serde_json::json!({
            "format":"clap", "stage":"probe", "pluginId":"clap:fixture", "message":"child timed out"
        })
    );
    assert_eq!(
        serde_json::from_value::<ExternalProcessorCatalog>(encoded).unwrap(),
        catalog
    );
}

#[test]
fn graph_apply_explicit_control_intent_extends_the_bare_document() {
    let document = mixrelay_audio_graph::GraphDocument::new(2, vec![], vec![]);
    let bare = serde_json::to_value(&document).unwrap();
    let mut request: GraphApplyRequest = serde_json::from_value(bare.clone()).unwrap();
    assert!(request.processor_parameter_changes.is_empty());
    assert_eq!(serde_json::to_value(&request).unwrap(), bare);
    request
        .processor_parameter_changes
        .push(GraphProcessorParameterChange {
            node_id: "effect".into(),
            type_id: "vst3:fixture".into(),
            parameter_id: 42,
            value: 0.25,
        });
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(
        encoded["processorParameterChanges"][0],
        serde_json::json!({
            "nodeId":"effect", "typeId":"vst3:fixture", "parameterId":42, "value":0.25
        })
    );
    assert_eq!(
        serde_json::from_value::<GraphApplyRequest>(encoded.clone()).unwrap(),
        request
    );
    assert_eq!(
        serde_json::from_value::<mixrelay_audio_graph::GraphDocument>(encoded.clone()).unwrap(),
        document,
        "desktop preflight can inspect the document while forwarding the complete intent unchanged"
    );
    let mut targeted = encoded.clone();
    targeted["expectedGraphId"] = "graph-1".into();
    assert_eq!(
        serde_json::from_value::<GraphApplyRequest>(targeted)
            .unwrap()
            .expected_graph_id
            .as_deref(),
        Some("graph-1")
    );
    let mut malformed = encoded;
    malformed["processorParameterChanges"][0]["stateBase64"] = "implicit preset replacement".into();
    assert!(serde_json::from_value::<GraphApplyRequest>(malformed).is_err());
}
