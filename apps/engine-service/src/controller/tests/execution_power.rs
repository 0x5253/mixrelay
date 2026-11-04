//! Engine power, pause persistence and execution ownership across service restarts.
use super::*;
use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};
use mixrelay_control_server::ControlBackend;

const SAVED_PAUSE: &str = "engine-paused.json";

fn recorder_request(action: RecorderAction) -> RecorderRequest {
    RecorderRequest {
        action,
        path: None,
        sample_rate: None,
        encoding: None,
    }
}

fn graph(revision: u64) -> GraphDocument {
    serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":revision,
        "nodes":[{"id":"record","type":"recorder","config":{"channels":2}}],
        "edges":[]
    }))
    .unwrap()
}

#[test]
fn a_pause_outlives_the_service_until_resume() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-saved-pause").unwrap();
    let catalog = directory.join("graphs.json");
    let marker = directory.join(SAVED_PAUSE);
    let start = || {
        controller(endpoint_document(1, "Retained device")).with_graph_catalog_path(catalog.clone())
    };
    let engine = start();
    assert!(engine.core.status().engine_power.unwrap().enabled);
    let inventory = engine
        .core
        .status()
        .audio_session
        .unwrap()
        .virtual_endpoint_settings
        .unwrap();
    let nodes = engine.graph().nodes;
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    settle(&engine);
    assert!(marker.is_file());
    drop(engine);
    let restarted = start();
    assert!(!restarted.core.status().engine_power.unwrap().enabled);
    // The audio worker takes the restored graph before the user resumes.
    let paused = settle(&restarted);
    assert!(paused.plan().source_bindings().is_empty());
    assert!(paused.plan().sink_bindings().is_empty());
    assert_eq!(restarted.graph().nodes, nodes);
    assert_eq!(
        restarted
            .core
            .status()
            .audio_session
            .unwrap()
            .virtual_endpoint_settings
            .unwrap(),
        inventory
    );
    // Relaunching the whole controller repeats durable startup, without resuming audio.
    drop(paused);
    drop(restarted);
    let restarted = start();
    let paused = settle(&restarted);
    assert!(!restarted.core.status().engine_power.unwrap().enabled);
    assert!(paused.plan().source_bindings().is_empty());
    assert_eq!(
        restarted
            .core
            .status()
            .audio_session
            .unwrap()
            .virtual_endpoint_settings
            .unwrap(),
        inventory
    );
    let resumed = restarted
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    assert_eq!(resumed.persistence_error, None);
    let running = settle(&restarted);
    assert!(!running.plan().source_bindings().is_empty());
    assert!(!running.plan().sink_bindings().is_empty());
    assert_eq!(restarted.graph().nodes, nodes);
    assert!(!marker.exists());
    drop(restarted);
    assert!(start().core.status().engine_power.unwrap().enabled);
}

#[test]
fn a_pause_after_repeated_failures_keeps_its_reason_until_resume() {
    use mixrelay_control_protocol::EnginePauseReason;
    let directory = mixrelay_test_support::FixtureDir::new("engine-crash-loop").unwrap();
    let catalog = directory.join("graphs.json");
    let start = |tripped| {
        controller(graph(1))
            .after_repeated_failures(tripped)
            .with_graph_catalog_path(catalog.clone())
    };
    let engine = start(true);
    let status = engine.core.status().engine_power.unwrap();
    assert!(!status.enabled);
    assert_eq!(
        status.pause_reason,
        Some(EnginePauseReason::RepeatedFailures)
    );
    drop(engine);
    // A later clean start keeps both the pause and why it happened.
    let restarted = start(false);
    assert_eq!(
        restarted.core.status().engine_power.unwrap().pause_reason,
        Some(EnginePauseReason::RepeatedFailures)
    );
    settle(&restarted);
    restarted
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    settle(&restarted);
    assert_eq!(
        restarted.core.status().engine_power.unwrap().pause_reason,
        None
    );
    // A user pause carries no reason.
    restarted
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    settle(&restarted);
    drop(restarted);
    let paused = start(false);
    assert!(!paused.core.status().engine_power.unwrap().enabled);
    assert_eq!(
        paused.core.status().engine_power.unwrap().pause_reason,
        None
    );
}

#[test]
fn an_unsaved_pause_choice_is_reported_until_maintenance_saves_it() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-unsaved-pause").unwrap();
    let engine = controller(graph(1)).with_graph_catalog_path(directory.join("graphs.json"));
    let marker = directory.join(SAVED_PAUSE);
    let warning = || engine.graph_catalog().unwrap().persistence_error;
    let unsaved = |warning: Option<String>| {
        assert!(warning.unwrap().starts_with("engine pause: "));
    };
    // A directory in the marker's place can be neither replaced nor removed.
    std::fs::create_dir(&marker).unwrap();
    unsaved(
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: false,
                expected_revision: None,
            })
            .unwrap()
            .persistence_error,
    );
    assert!(!engine.core.status().engine_power.unwrap().enabled);
    settle(&engine);
    // A successful catalog save does not clear the unsaved pause.
    engine
        .update_graph_layout(
            &engine.core.graph_catalog().unwrap().active_id,
            mixrelay_control_protocol::GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: mixrelay_control_protocol::GraphLayout::default(),
            },
        )
        .unwrap();
    unsaved(warning());
    engine.core.test_retry_pause_save();
    unsaved(warning());
    assert!(engine.core.status().engine_power.unwrap().unsaved);
    std::fs::remove_dir(&marker).unwrap();
    // A later successful change does not report the earlier failure.
    let resumed = engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    assert_eq!(resumed.persistence_error, None);
    settle(&engine);
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    settle(&engine);
    assert!(marker.is_file());
    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    unsaved(
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None,
            })
            .unwrap()
            .persistence_error,
    );
    settle(&engine);
    std::fs::remove_dir(&marker).unwrap();
    engine.core.test_retry_pause_save();
    assert_eq!(warning(), None);
    assert!(!engine.core.status().engine_power.unwrap().unsaved);
    assert!(!marker.exists());
    // Nothing left to retry.
    engine.core.test_retry_pause_save();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one native take lifecycle retains its ordered PCM and persistence assertions"
)]
fn pause_retires_render_taps_but_resumes_the_same_rf64_without_the_gap() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-pause-recording").unwrap();
    let path = directory.join("take.wav");
    let engine = controller(graph(1)).with_graph_catalog_path(directory.join("graphs.json"));
    let catalog = engine.graph_catalog().unwrap();
    let id = engine
        .core
        .test_recorder_session_id(&catalog.active_id, "record")
        .unwrap();
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[], 48_000);
    engine
        .recorders
        .command(
            &id,
            RecorderRequest {
                path: Some(path.to_string_lossy().into_owned()),
                encoding: Some(mixrelay_audio_io::recording::Encoding::Float32),
                ..recorder_request(RecorderAction::Record)
            },
        )
        .unwrap();
    let first = [0.25, -0.5, 0.125, -0.25];
    let second = [-0.75, 0.5, -0.125, 0.25];
    for _ in 0..3 {
        tap.capture(&first, 48_000);
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: false,
                expected_revision: None,
            })
            .unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        engine.core.test_reclaim_recorders();
        let paused = engine.recorders.status(&id);
        assert_eq!(paused.state, RecorderState::Paused);
        assert_eq!(paused.path.as_deref(), path.to_str());
        tap.capture(&[0.99; 16], 48_000);
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: false,
                expected_revision: None,
            })
            .unwrap();
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None,
            })
            .unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        assert_eq!(engine.recorders.status(&id).state, RecorderState::Recording);
        tap.capture(&second, 48_000);
    }
    // Simulate complete plan/tap retirement while a take is paused.
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    drop(tap);
    engine.core.test_reclaim_recorders();
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Paused);
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    engine
        .recorders
        .command(&id, recorder_request(RecorderAction::Stop))
        .unwrap();
    let mut file = mixrelay_test_support::rf64::open(&path).unwrap();
    let samples: Vec<f32> = file.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, [first, second].concat().repeat(3));
    assert_eq!(
        mixrelay_test_support::rf64::inspect(&path).unwrap().frames,
        12
    );
}

#[test]
fn global_resume_preserves_manual_pause_and_stopping_while_paused_finalizes() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-manual-pause").unwrap();
    let path = directory.join("take.wav");
    let engine = controller(graph(1)).with_graph_catalog_path(directory.join("graphs.json"));
    let catalog = engine.graph_catalog().unwrap();
    let id = engine
        .core
        .test_recorder_session_id(&catalog.active_id, "record")
        .unwrap();
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[], 48_000);
    engine
        .recorders
        .command(
            &id,
            RecorderRequest {
                path: Some(path.to_string_lossy().into_owned()),
                ..recorder_request(RecorderAction::Record)
            },
        )
        .unwrap();
    tap.capture(&[0.25; 4], 48_000);
    engine
        .recorders
        .command(&id, recorder_request(RecorderAction::Pause))
        .unwrap();
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    drop(tap);
    engine.core.test_reclaim_recorders();
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Paused);
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Paused);
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    engine.core.test_reclaim_recorders();
    engine.recorders.stop_all().unwrap();
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Stopped);
    assert_eq!(
        mixrelay_test_support::rf64::inspect(&path).unwrap().frames,
        2
    );
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Stopped);
}

#[test]
fn failed_resume_keeps_the_open_take_paused() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-resume-failure").unwrap();
    let path = directory.join("take.wav");
    let engine = controller(graph(1)).with_graph_catalog_path(directory.join("graphs.json"));
    let catalog = engine.graph_catalog().unwrap();
    let id = engine
        .core
        .test_recorder_session_id(&catalog.active_id, "record")
        .unwrap();
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    let tap = engine.recorders.prepare(&id, 2).unwrap();
    tap.capture(&[], 48_000);
    engine
        .recorders
        .command(
            &id,
            RecorderRequest {
                path: Some(path.to_string_lossy().into_owned()),
                ..recorder_request(RecorderAction::Record)
            },
        )
        .unwrap();
    tap.capture(&[0.25; 4], 48_000);
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    assert!(engine.exchange.activate_pending(&mut processor));
    drop(tap);
    engine.core.test_reclaim_recorders();
    engine.core.test_graph().write().nodes[0].config =
        mixrelay_audio_graph::NodeConfig::Recorder { channels: 0 };
    assert!(
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None
            })
            .is_err()
    );
    assert!(!engine.core.status().engine_power.unwrap().enabled);
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Paused);
    engine.core.test_graph().write().nodes[0].config =
        mixrelay_audio_graph::NodeConfig::Recorder { channels: 2 };
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: true,
            expected_revision: None,
        })
        .unwrap();
    assert_eq!(engine.recorders.status(&id).state, RecorderState::Recording);
    engine.recorders.stop_all().unwrap();
    assert_eq!(
        mixrelay_test_support::rf64::inspect(&path).unwrap().frames,
        2
    );
}

#[test]
fn off_edits_keep_authored_catalog_and_resume_the_latest_document() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-power").unwrap();
    let engine = controller(graph(1)).with_graph_catalog_path(directory.join("graphs.json"));
    let original = engine.graph_catalog().unwrap();
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    for _ in 0..3 {
        let before = engine.graph();
        let off = engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: false,
                expected_revision: None,
            })
            .unwrap();
        assert!(!engine.core.status().engine_power.unwrap().enabled);
        assert!(engine.core.status().engine_power.unwrap().transitioning);
        assert_eq!(
            engine
                .core
                .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                    enabled: false,
                    expected_revision: None
                })
                .unwrap()
                .revision,
            off.revision
        );
        assert!(
            engine
                .core
                .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                    enabled: true,
                    expected_revision: None
                })
                .is_err()
        );
        assert!(engine.exchange.activate_pending(&mut processor));
        engine.core.test_reclaim_recorders();
        assert!(!engine.core.status().engine_power.unwrap().transitioning);
        assert!(
            engine
                .application_capture_manifest()
                .unwrap()
                .sources
                .is_empty()
        );
        let catalog = engine.graph_catalog().unwrap();
        assert_eq!(catalog.active_id, original.active_id);
        assert_eq!(catalog.document.nodes, before.nodes);
        assert_eq!(catalog.document.edges, original.document.edges);
        let mut edited = engine.graph();
        edited.revision += 1;
        edited.nodes[0].label = Some("Saved while off".into());
        engine.apply_graph_request(edited.into()).unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        let composed = engine
            .core
            .test_composed_document(&engine.graph(), engine.core.test_context())
            .unwrap();
        assert!(composed.nodes.is_empty());
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None,
            })
            .unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        assert!(engine.core.status().engine_power.unwrap().enabled);
        assert_eq!(
            engine.graph().nodes[0].label.as_deref(),
            Some("Saved while off")
        );
    }
}

#[test]
fn failed_enable_keeps_execution_off() {
    let engine = controller(GraphDocument::new(1, vec![], vec![]));
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    engine.exchange.activate_pending(&mut processor);
    // Authored invalid processor cannot be silently activated on power-on.
    engine.core.test_graph().write().nodes = graph(1).nodes;
    engine.core.test_graph().write().nodes[0].config =
        mixrelay_audio_graph::NodeConfig::Recorder { channels: 0 };
    assert!(
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None
            })
            .is_err()
    );
    assert!(!engine.core.status().engine_power.unwrap().enabled);
}

#[test]
fn storage_failure_cannot_block_stop_and_stale_start_preflight_cannot_publish() {
    let directory = mixrelay_test_support::FixtureDir::new("engine-power-disk").unwrap();
    let engine = controller(graph(1)).with_graph_catalog_path(directory.join("graphs.json"));
    let mut processor = engine
        .core
        .test_prepare_processor(&engine.graph(), engine.core.test_context())
        .unwrap();
    let blocker = directory.join("file-not-directory");
    std::fs::write(&blocker, b"occupied").unwrap();
    engine
        .core
        .test_set_catalog_path(blocker.join("graphs.json"));
    let response = engine
        .core
        .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
            enabled: false,
            expected_revision: None,
        })
        .unwrap();
    assert!(response.persistence_error.is_some());
    engine.exchange.activate_pending(&mut processor);
    assert!(!engine.core.status().engine_power.unwrap().enabled);
    let preview = engine.preview_engine_start().unwrap();
    assert_eq!(preview.revision, response.revision + 1);
    assert!(
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: Some(response.revision - 1)
            })
            .is_err()
    );
    assert!(!engine.core.status().engine_power.unwrap().enabled);
}
