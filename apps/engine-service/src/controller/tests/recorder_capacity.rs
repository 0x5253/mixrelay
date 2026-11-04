use super::*;
use mixrelay_audio_io::recording::{Encoding, RecorderAction, RecorderRequest, RecorderState};
use mixrelay_control_protocol::RecorderControlRequest;
use mixrelay_test_support::FixtureDir;

fn node(id: &str, channels: u16) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::Recorder { channels },
    }
}
fn command(action: RecorderAction, path: Option<String>) -> RecorderControlRequest {
    RecorderControlRequest {
        expected_revision: 1,
        expected_recording: None,
        request: RecorderRequest {
            action,
            path,
            sample_rate: None,
            encoding: Some(Encoding::Float32),
        },
    }
}

#[test]
fn multichannel_recording_can_pause_resume_stop_and_start_a_new_take() {
    let fixture = FixtureDir::new("multichannel-takes").unwrap();
    let engine = controller(GraphDocument::new(1, vec![node("record", 8)], vec![]))
        .with_graph_catalog_path(fixture.join("graphs.json"));
    let id = engine
        .core
        .test_recorder_session_id("graph-1", "record")
        .unwrap();
    let tap = engine.recorders.prepare(&id, 8).unwrap();
    tap.capture(&[], 48_000);
    for name in ["first.wav", "second.wav"] {
        let path = fixture.join(name);
        assert_eq!(
            engine
                .control_recorder(
                    "graph-1",
                    "record",
                    command(RecorderAction::Record, Some(path.to_str().unwrap().into()))
                )
                .unwrap()
                .state,
            RecorderState::Recording
        );
        tap.capture(&[0.25; 8 * 256], 48_000);
        assert_eq!(
            engine
                .control_recorder("graph-1", "record", command(RecorderAction::Pause, None))
                .unwrap()
                .state,
            RecorderState::Paused
        );
        engine
            .control_recorder("graph-1", "record", command(RecorderAction::Record, None))
            .unwrap();
        tap.capture(&[0.5; 8 * 256], 48_000);
        assert_eq!(
            engine
                .control_recorder("graph-1", "record", command(RecorderAction::Stop, None))
                .unwrap()
                .frames,
            512
        );
        let reader = mixrelay_test_support::rf64::open(path).unwrap();
        assert_eq!(reader.spec().channels, 8);
        assert_eq!(reader.duration(), 512);
    }
}

#[test]
fn multiple_recorders_can_record_while_another_take_is_paused() {
    let fixture = FixtureDir::new("multiple-recorders").unwrap();
    let engine = controller(GraphDocument::new(
        1,
        vec![node("first", 2), node("second", 3)],
        vec![],
    ))
    .with_graph_catalog_path(fixture.join("graphs.json"));
    let first_id = engine
        .core
        .test_recorder_session_id("graph-1", "first")
        .unwrap();
    let second_id = engine
        .core
        .test_recorder_session_id("graph-1", "second")
        .unwrap();
    let first = engine.recorders.prepare(&first_id, 2).unwrap();
    let second = engine.recorders.prepare(&second_id, 3).unwrap();
    first.capture(&[], 48_000);
    second.capture(&[], 48_000);
    engine
        .control_recorder(
            "graph-1",
            "first",
            command(
                RecorderAction::Record,
                Some(fixture.join("first.wav").to_str().unwrap().into()),
            ),
        )
        .unwrap();
    first.capture(&[0.25; 2], 48_000);
    engine
        .control_recorder(
            "graph-1",
            "second",
            command(
                RecorderAction::Record,
                Some(fixture.join("second.wav").to_str().unwrap().into()),
            ),
        )
        .unwrap();
    engine
        .control_recorder("graph-1", "first", command(RecorderAction::Pause, None))
        .unwrap();
    second.capture(&[0.75; 6], 48_000);
    assert_eq!(
        engine
            .control_recorder("graph-1", "second", command(RecorderAction::Stop, None))
            .unwrap()
            .frames,
        2
    );
    assert_eq!(
        engine.recorder_status("graph-1", "first").unwrap().state,
        RecorderState::Paused
    );
    assert_eq!(
        engine
            .control_recorder("graph-1", "first", command(RecorderAction::Stop, None))
            .unwrap()
            .frames,
        1
    );
    assert_eq!(
        mixrelay_test_support::rf64::open(fixture.join("second.wav"))
            .unwrap()
            .spec()
            .channels,
        3
    );
}
