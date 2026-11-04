use super::*;
use mixrelay_audio_graph::{GraphDocument, NodeId, NodeSpec};
use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};
use mixrelay_control_protocol::{GraphCatalogAction, GraphCatalogRequest, RecorderControlRequest};
use mixrelay_control_server::ControlBackend;

fn command(revision: u64, action: RecorderAction, path: Option<String>) -> RecorderControlRequest {
    RecorderControlRequest {
        expected_recording: None,
        expected_revision: revision,
        request: RecorderRequest {
            action,
            path,
            sample_rate: None,
            encoding: None,
        },
    }
}

#[test]
fn channels_can_change_only_after_the_take_is_stopped() {
    let directory = mixrelay_test_support::FixtureDir::new("recorder-channel-control").unwrap();
    let document = GraphDocument::new(
        1,
        vec![NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("record"),
            label: None,
            config: NodeConfig::Recorder { channels: 2 },
        }],
        vec![],
    );
    let engine = controller(document).with_graph_catalog_path(directory.join("graphs.json"));
    let catalog = engine.graph_catalog().unwrap();
    let graph_id = catalog.active_id;
    let id = engine
        .core
        .test_recorder_session_id(&graph_id, "record")
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[0.0; 4], 48_000);
    engine
        .control_recorder(
            &graph_id,
            "record",
            command(
                catalog.document.revision,
                RecorderAction::Record,
                Some(directory.join("take.wav").to_string_lossy().into_owned()),
            ),
        )
        .unwrap();
    let mut changed = catalog.document;
    changed.revision += 1;
    changed.nodes[0].config = NodeConfig::Recorder { channels: 8 };
    assert_eq!(
        engine
            .apply_graph_request(changed.clone().into())
            .unwrap_err()
            .code,
        ErrorCode::RecorderFailed
    );
    engine
        .control_recorder(&graph_id, "record", command(1, RecorderAction::Pause, None))
        .unwrap();
    assert_eq!(
        engine
            .apply_graph_request(changed.clone().into())
            .unwrap_err()
            .code,
        ErrorCode::RecorderFailed
    );
    engine
        .control_recorder(&graph_id, "record", command(1, RecorderAction::Stop, None))
        .unwrap();
    assert!(engine.apply_graph_request(changed.into()).is_ok());
}

#[test]
fn controls_target_original_graph_even_after_another_graph_is_selected() {
    let directory = mixrelay_test_support::FixtureDir::new("recorder-control").unwrap();
    let document = GraphDocument::new(
        1,
        vec![NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("record"),
            label: None,
            config: NodeConfig::Recorder { channels: 2 },
        }],
        vec![],
    );
    let engine = controller(document).with_graph_catalog_path(directory.join("graphs.json"));
    let original = engine.graph_catalog().unwrap();
    let id = mixrelay_render::recording_session_key(
        &mixrelay_audio_graph::composition::runtime_node_id(
            &original.active_id,
            &NodeId::from("record"),
        ),
        2,
    );
    let tap = engine.recorders.prepare(id.as_str(), 2).unwrap();
    tap.capture(&[0.0; 4], 48_000);
    let path = directory.join("take.wav").to_string_lossy().into_owned();
    engine
        .control_recorder(
            &original.active_id,
            "record",
            command(
                original.document.revision,
                RecorderAction::Record,
                Some(path),
            ),
        )
        .unwrap();
    let second = engine
        .mutate_graph_catalog(GraphCatalogRequest {
            expected_revision: original.document.revision,
            expected_catalog_revision: original.catalog_revision,
            action: GraphCatalogAction::Create {
                name: "Another graph".to_owned(),
            },
        })
        .unwrap();
    assert_ne!(original.active_id, second.active_id);
    assert_eq!(
        engine
            .recorder_status(&original.active_id, "record")
            .unwrap()
            .state,
        RecorderState::Recording
    );
    assert!(engine.recorder_status(&second.active_id, "record").is_err());
    assert!(
        engine
            .control_recorder(
                &original.active_id,
                "missing",
                command(second.document.revision, RecorderAction::Stop, None)
            )
            .is_err()
    );
    tap.capture(&[0.25; 4], 48_000);
    let stopped = engine
        .control_recorder(
            &original.active_id,
            "record",
            command(second.document.revision, RecorderAction::Stop, None),
        )
        .unwrap();
    assert_eq!(stopped.state, RecorderState::Stopped);
    assert_eq!(stopped.frames, 2);
}
