use crate::controller_test_support::settle;
use crate::endpoint_settings_tests::controller;
use crate::*;
use mixrelay_control_protocol::{GraphCatalogAction, GraphCatalogRequest};

struct Fixture(mixrelay_test_support::FixtureDir);
impl Fixture {
    fn new() -> Self {
        Self(mixrelay_test_support::FixtureDir::new("catalog-recorder").unwrap())
    }
    fn path(&self) -> std::path::PathBuf {
        self.0.join("graphs.json")
    }
}
fn request(controller: &EngineGraphController, action: GraphCatalogAction) -> GraphCatalogRequest {
    GraphCatalogRequest {
        expected_revision: controller.graph().revision,
        expected_catalog_revision: controller.graph_catalog().unwrap().catalog_revision,
        action,
    }
}
fn remove(
    engine: &EngineGraphController,
    id: &str,
) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
    engine.mutate_graph_catalog(request(
        engine,
        GraphCatalogAction::Delete { id: id.into() },
    ))
}

#[test]
fn removal_finalizes_recording_and_late_recorder_command_cannot_target_replacement() {
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest};
    use mixrelay_control_protocol::RecorderControlRequest;
    let fixture = Fixture::new();
    let graph: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,
        "nodes":[{"id":"record","type":"recorder","config":{"channels":2}}],"edges":[]
    }))
    .unwrap();
    let engine = controller(graph).with_graph_catalog_path(fixture.path());
    let before = engine.graph_catalog().unwrap();
    let replacement = engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Create {
                name: "Mono recorder".into(),
            },
        ))
        .unwrap();
    let mono: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":before.document.revision + 1,
        "nodes":[{"id":"record","type":"recorder","config":{"channels":1}}],"edges":[]
    }))
    .unwrap();
    engine.apply_graph_request(mono.clone().into()).unwrap();
    settle(&engine);
    engine
        .mutate_graph_catalog(request(
            &engine,
            GraphCatalogAction::Select {
                id: before.active_id.clone(),
            },
        ))
        .unwrap();
    let id = engine
        .core
        .test_recorder_session_id(&before.active_id, "record")
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[0.25; 8], 48_000);
    let command = RecorderControlRequest {
        expected_recording: None,
        expected_revision: engine.graph().revision,
        request: RecorderRequest {
            action: RecorderAction::Record,
            path: Some(fixture.0.join("take.wav").to_string_lossy().into_owned()),
            sample_rate: None,
            encoding: None,
        },
    };
    engine
        .control_recorder(&before.active_id, "record", command.clone())
        .unwrap();
    tap.capture(&[0.25; 8], 48_000);
    remove(&engine, &before.active_id).unwrap();
    assert_eq!(
        engine.recorders.status(&id).state,
        mixrelay_audio_io::recording::RecorderState::Stopped
    );
    settle(&engine);
    assert_eq!(
        engine.graph_catalog().unwrap().active_id,
        replacement.active_id
    );
    assert_eq!(engine.graph().nodes, mono.nodes);
    assert!(
        engine
            .control_recorder(&before.active_id, "record", command)
            .is_err()
    );
    let metadata = mixrelay_test_support::rf64::inspect(fixture.0.join("take.wav")).unwrap();
    assert!(metadata.frames > 0);
    assert_eq!(metadata.data_bytes, metadata.frames * 2 * 3);
}
