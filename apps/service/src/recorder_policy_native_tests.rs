//! Native recorder and document lifecycle assertions preserved at the application boundary.
#[cfg(test)]
mod execution_power {
    const SAVED_PAUSE: &str = "engine-paused.json";
    use mixrelay_audio_graph::GraphDocument;
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};
    use mixrelay_control_server::ControlBackend;

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
        use crate::controller_test_support::settle;
        let directory = mixrelay_test_support::FixtureDir::new("engine-saved-pause").unwrap();
        let catalog = directory.join("graphs.json");
        let marker = directory.join(SAVED_PAUSE);
        let start = || {
            crate::endpoint_settings_tests::controller(
                crate::endpoint_settings_tests::endpoint_document(1, "Retained device"),
            )
            .with_graph_catalog_path(catalog.clone())
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
        use crate::controller_test_support::settle;
        use mixrelay_control_protocol::EnginePauseReason;
        let directory = mixrelay_test_support::FixtureDir::new("engine-crash-loop").unwrap();
        let catalog = directory.join("graphs.json");
        let start = |tripped| {
            crate::endpoint_settings_tests::controller(graph(1))
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
        use crate::controller_test_support::settle;
        let directory = mixrelay_test_support::FixtureDir::new("engine-unsaved-pause").unwrap();
        let engine = crate::endpoint_settings_tests::controller(graph(1))
            .with_graph_catalog_path(directory.join("graphs.json"));
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
        let engine = crate::endpoint_settings_tests::controller(graph(1))
            .with_graph_catalog_path(directory.join("graphs.json"));
        let catalog = engine.graph_catalog().unwrap();
        let id = engine
            .core
            .test_recorder_session_id(&catalog.active_id, "record")
            .unwrap();
        let mut processor = engine
            .core
            .test_prepare_processor(&engine.graph(), engine.core.test_context())
            .unwrap();
        let tap = engine.recording_registry.prepare(&id, 2).unwrap();
        tap.capture(&[], 48_000);
        engine
            .recording_registry
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
            let paused = engine.recording_registry.status(&id);
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
            assert_eq!(
                engine.recording_registry.status(&id).state,
                RecorderState::Recording
            );
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
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Paused
        );
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None,
            })
            .unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        engine
            .recording_registry
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
    #[expect(
        clippy::too_many_lines,
        reason = "one native take lifecycle retains its ordered PCM and persistence assertions"
    )]
    fn global_resume_preserves_manual_pause_and_stopping_while_paused_finalizes() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-manual-pause").unwrap();
        let path = directory.join("take.wav");
        let engine = crate::endpoint_settings_tests::controller(graph(1))
            .with_graph_catalog_path(directory.join("graphs.json"));
        let catalog = engine.graph_catalog().unwrap();
        let id = engine
            .core
            .test_recorder_session_id(&catalog.active_id, "record")
            .unwrap();
        let mut processor = engine
            .core
            .test_prepare_processor(&engine.graph(), engine.core.test_context())
            .unwrap();
        let tap = engine.recording_registry.prepare(&id, 2).unwrap();
        tap.capture(&[], 48_000);
        engine
            .recording_registry
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
            .recording_registry
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
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Paused
        );
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None,
            })
            .unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Paused
        );
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: false,
                expected_revision: None,
            })
            .unwrap();
        assert!(engine.exchange.activate_pending(&mut processor));
        engine.core.test_reclaim_recorders();
        engine.recording_registry.stop_all().unwrap();
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Stopped
        );
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
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Stopped
        );
    }

    #[test]
    fn failed_resume_keeps_the_open_take_paused() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-resume-failure").unwrap();
        let path = directory.join("take.wav");
        let engine = crate::endpoint_settings_tests::controller(graph(1))
            .with_graph_catalog_path(directory.join("graphs.json"));
        let catalog = engine.graph_catalog().unwrap();
        let id = engine
            .core
            .test_recorder_session_id(&catalog.active_id, "record")
            .unwrap();
        let mut processor = engine
            .core
            .test_prepare_processor(&engine.graph(), engine.core.test_context())
            .unwrap();
        let tap = engine.recording_registry.prepare(&id, 2).unwrap();
        tap.capture(&[], 48_000);
        engine
            .recording_registry
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
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Paused
        );
        engine.core.test_graph().write().nodes[0].config =
            mixrelay_audio_graph::NodeConfig::Recorder { channels: 2 };
        engine
            .core
            .set_engine_power(mixrelay_control_protocol::EnginePowerRequest {
                enabled: true,
                expected_revision: None,
            })
            .unwrap();
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Recording
        );
        engine.recording_registry.stop_all().unwrap();
        assert_eq!(
            mixrelay_test_support::rf64::inspect(&path).unwrap().frames,
            2
        );
    }

    #[test]
    fn off_edits_keep_authored_catalog_and_resume_the_latest_document() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-power").unwrap();
        let engine = crate::endpoint_settings_tests::controller(graph(1))
            .with_graph_catalog_path(directory.join("graphs.json"));
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
        let engine =
            crate::endpoint_settings_tests::controller(GraphDocument::new(1, vec![], vec![]));
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
        let engine = crate::endpoint_settings_tests::controller(graph(1))
            .with_graph_catalog_path(directory.join("graphs.json"));
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
}

#[cfg(test)]
mod recorder_control {
    use crate::*;
    use mixrelay_audio_graph::{GraphDocument, NodeId, NodeSpec};
    use mixrelay_audio_io::recording::{RecorderAction, RecorderRequest, RecorderState};
    use mixrelay_control_protocol::{
        GraphCatalogAction, GraphCatalogRequest, RecorderControlRequest,
    };
    use mixrelay_control_server::ControlBackend;

    fn command(
        revision: u64,
        action: RecorderAction,
        path: Option<String>,
    ) -> RecorderControlRequest {
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
        let engine = crate::endpoint_settings_tests::controller(document)
            .with_graph_catalog_path(directory.join("graphs.json"));
        let catalog = engine.graph_catalog().unwrap();
        let graph_id = catalog.active_id;
        let id = engine
            .core
            .test_recorder_session_id(&graph_id, "record")
            .unwrap();
        let tap = engine.recording_registry.prepare(&id, 2).unwrap();
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
        let engine = crate::endpoint_settings_tests::controller(document)
            .with_graph_catalog_path(directory.join("graphs.json"));
        let original = engine.graph_catalog().unwrap();
        let id = mixrelay_audio_engine::recording_session_key(
            &graph_composition::runtime_node_id(&original.active_id, &NodeId::from("record")),
            2,
        );
        let tap = engine.recording_registry.prepare(id.as_str(), 2).unwrap();
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
}

#[cfg(test)]
mod recorder_capacity {
    use crate::endpoint_settings_tests::controller;
    use crate::*;
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
        let tap = engine.recording_registry.prepare(&id, 8).unwrap();
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
        let first = engine.recording_registry.prepare(&first_id, 2).unwrap();
        let second = engine.recording_registry.prepare(&second_id, 3).unwrap();
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
}

#[cfg(test)]
mod node_recording {
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
        engine: &EngineGraphController,
        processor: &mut Box<mixrelay_audio_engine::BlockProcessor>,
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
    #[expect(
        clippy::too_many_lines,
        reason = "one native take lifecycle retains its ordered PCM and persistence assertions"
    )]
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
        let tap = engine.recording_registry.prepare(&id, 2).unwrap();
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
        assert_eq!(
            engine.recording_registry.status(&id).state,
            RecorderState::Recording
        );
        assert_eq!(engine.recording_registry.status(&id).path, b.path);
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
        let tap = engine.recording_registry.prepare(&id, 2).unwrap();
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
}
