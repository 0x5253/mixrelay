use crate::*;
use mixrelay_control_protocol::NodeControlRequest;

fn graph() -> GraphDocument {
    serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,
        "nodes":[{"id":"gain","type":"gain","config":{"channels":2,"gainDb":-3.0}},
            {"id":"record","type":"recorder","config":{"channels":2}}],
        "edges":[{"id":"wire","source":{"nodeId":"gain","portId":"audio"},
            "destination":{"nodeId":"record","portId":"audio"}}]
    }))
    .unwrap()
}

fn request(revision: u64, enabled: Option<bool>, muted: Option<bool>) -> NodeControlRequest {
    NodeControlRequest {
        expected_revision: revision,
        enabled,
        muted,
        gain_db: None,
    }
}

fn acknowledge_control(
    engine: &crate::controller_test_support::ControllerFixture,
    processor: &mut Box<mixrelay_render::BlockProcessor>,
) {
    let plan = engine.exchange.plan_revisions();
    assert!(
        !engine.exchange.activate_pending(processor),
        "control must not replace topology"
    );
    processor
        .process_block(128, &[], &mut [])
        .expect("headless audio block");
    assert_eq!(engine.exchange.plan_revisions(), plan);
    let revision = engine.exchange.revisions();
    assert_eq!(
        revision.active, revision.submitted,
        "audio block acknowledges control state"
    );
}

#[test]
fn previous_take_stop_cannot_stop_a_new_recording_at_same_graph_revision() {
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};
    use mixrelay_control_protocol::{ObservedRecording, RecorderControlRequest};
    let directory = mixrelay_test_support::FixtureDir::new("take-fence").unwrap();
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_graph_catalog_path(directory.join("graphs.json"));
    let graph_id = engine.graph_catalog().unwrap().active_id;
    let id = engine
        .core
        .test_recorder_session_id(&graph_id, "record")
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[0.25; 8], 48_000);
    let revision = engine.exchange.revisions().submitted;
    let command = |action, path, expected_recording| RecorderControlRequest {
        expected_revision: revision,
        expected_recording,
        request: RecorderRequest {
            action,
            path,
            sample_rate: None,
            encoding: None,
        },
    };
    let a = engine
        .control_recorder(
            &graph_id,
            "record",
            command(
                RecorderAction::Record,
                Some(directory.join("a.wav").to_string_lossy().into_owned()),
                None,
            ),
        )
        .unwrap();
    let observed = ObservedRecording {
        state: a.state,
        path: a.path,
    };
    engine
        .control_recorder(
            &graph_id,
            "record",
            command(RecorderAction::Stop, None, Some(observed.clone())),
        )
        .unwrap();
    let b = engine
        .control_recorder(
            &graph_id,
            "record",
            command(
                RecorderAction::Record,
                Some(directory.join("b.wav").to_string_lossy().into_owned()),
                None,
            ),
        )
        .unwrap();
    let error = engine
        .control_recorder(
            &graph_id,
            "record",
            command(RecorderAction::Stop, None, Some(observed)),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::GraphRevisionStale);
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Recording);
    assert_eq!(engine.recorders.status(&id).path, b.path);
    engine
        .control_recorder(
            &graph_id,
            "record",
            command(
                RecorderAction::Stop,
                None,
                Some(ObservedRecording {
                    state: b.state,
                    path: b.path,
                }),
            ),
        )
        .unwrap();
}
#[test]
fn recorder_off_finalizes_and_on_does_not_restart_recording() {
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};
    use mixrelay_control_protocol::RecorderControlRequest;
    let directory = mixrelay_test_support::FixtureDir::new("node-control-recorder").unwrap();
    let engine = crate::endpoint_settings_tests::controller(graph())
        .with_graph_catalog_path(directory.join("graphs.json"));
    let graph_id = engine.graph_catalog().unwrap().active_id;
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    let id = engine
        .core
        .test_recorder_session_id(&graph_id, "record")
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[0.25; 8], 48_000);
    let start = RecorderRequest {
        action: RecorderAction::Record,
        path: Some(directory.join("take.wav").to_string_lossy().into_owned()),
        sample_rate: None,
        encoding: None,
    };
    engine
        .control_recorder(
            &graph_id,
            "record",
            RecorderControlRequest {
                expected_recording: None,
                expected_revision: engine.exchange.revisions().submitted,
                request: start.clone(),
            },
        )
        .unwrap();
    tap.capture(&[0.25; 8], 48_000);
    let off = engine
        .core
        .update_node_control(
            &graph_id,
            "record",
            &request(engine.exchange.revisions().submitted, Some(false), None),
        )
        .unwrap();
    acknowledge_control(&engine, &mut processor);
    assert_eq!(
        engine.recorder_status(&graph_id, "record").unwrap().state,
        RecorderState::Stopped
    );
    assert_eq!(
        engine
            .control_recorder(
                &graph_id,
                "record",
                RecorderControlRequest {
                    expected_recording: None,
                    expected_revision: off.application.revision,
                    request: start,
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NodeDisabled
    );
    engine
        .core
        .update_node_control(
            &graph_id,
            "record",
            &request(off.application.revision, Some(true), None),
        )
        .unwrap();
    acknowledge_control(&engine, &mut processor);
    assert_eq!(
        engine.recorder_status(&graph_id, "record").unwrap().state,
        RecorderState::Stopped
    );
    assert!(std::fs::metadata(directory.join("take.wav")).unwrap().len() > 44);
}
