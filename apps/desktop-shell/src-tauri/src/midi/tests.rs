use super::*;
use mixrelay_midi_control::Device;
use state::Selection;

#[derive(Debug, Default)]
pub(crate) struct MemoryPreferences(Mutex<Option<Vec<u8>>>);
impl mixrelay_desktop_core::recording::PreferencesRepository for MemoryPreferences {
    fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
        Ok(lock(&self.0).clone())
    }
    fn replace(&self, bytes: &[u8]) -> std::io::Result<()> {
        *lock(&self.0) = Some(bytes.to_vec());
        Ok(())
    }
}
pub(in crate::midi) fn memory_state() -> State {
    State::with_repository(Arc::new(MemoryPreferences::default()))
}
pub(crate) fn fixture() -> (Arc<MemoryPreferences>, Midi) {
    let repository = Arc::new(MemoryPreferences::default());
    let (_, done) = tokio::sync::watch::channel(true);
    let midi = Midi {
        state: Arc::new(Mutex::new(State::with_repository(repository.clone()))),
        stop: Arc::new(AtomicBool::new(false)),
        done,
        operation_liveness: Arc::default(),
        write_gate: Arc::default(),
    };
    (repository, midi)
}

#[tokio::test]
async fn malformed_preferences_remain_in_storage_and_do_not_prevent_resident_audio_startup() {
    let repository = Arc::new(MemoryPreferences(Mutex::new(Some(b"corrupt".to_vec()))));
    let state = State::with_repository(repository.clone());
    assert!(
        state
            .unavailable
            .unwrap()
            .contains("MIDI preferences unavailable")
    );
    assert_eq!(lock(&repository.0).as_deref(), Some(b"corrupt".as_slice()));
}

#[tokio::test]
async fn shutdown_joins_the_worker_completion_after_resource_disposal() {
    struct Resource(Arc<AtomicBool>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let (_directory, mut midi) = fixture();
    let (completed, done) = tokio::sync::watch::channel(false);
    midi.done = done;
    let disposed = Arc::new(AtomicBool::new(false));
    let (release, released) = tokio::sync::oneshot::channel();
    let worker = std::thread::spawn({
        let disposed = Arc::clone(&disposed);
        move || {
            let resource = Resource(disposed);
            released.blocking_recv().unwrap();
            drop(resource);
            completed.send(true).unwrap();
        }
    });
    let shutdown = midi.shutdown();
    tokio::pin!(shutdown);
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(shutdown.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(midi.stop.load(Ordering::Acquire));
    assert!(!disposed.load(Ordering::Acquire));
    release.send(()).unwrap();
    shutdown.await.unwrap();
    assert!(disposed.load(Ordering::Acquire));
    worker.join().unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_stalled_midi_shutdown_returns_the_control_drain_failure_and_can_be_retried() {
    let (_directory, mut midi) = fixture();
    let (completed, done) = tokio::sync::watch::channel(false);
    midi.done = done;
    let retained = Arc::clone(&midi.state);
    let result = tokio::time::timeout(std::time::Duration::from_secs(36), midi.shutdown()).await;
    assert!(
        result.is_ok(),
        "shutdown must reach the existing bounded failure path"
    );
    assert_eq!(
        result.unwrap(),
        Err("Controls are still finishing; audio ownership was retained.".into())
    );
    assert!(Arc::ptr_eq(&retained, &midi.state));
    assert_eq!(Arc::strong_count(&retained), 2);
    completed.send(true).unwrap();
    midi.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn live_midi_operations_extend_the_existing_drain_until_resources_are_released() {
    let (_directory, mut midi) = fixture();
    let (completed, done) = tokio::sync::watch::channel(false);
    midi.done = done;
    let shutdown = midi.shutdown();
    tokio::pin!(shutdown);
    for _ in 0..3 {
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(30), &mut shutdown)
                .await
                .is_err()
        );
        midi.operation_liveness.notify_waiters();
    }
    completed.send(true).unwrap();
    shutdown.await.unwrap();
}

#[tokio::test]
async fn worker_completion_channel_failure_is_not_reported_as_successful_shutdown() {
    let (_directory, mut midi) = fixture();
    let (completed, done) = tokio::sync::watch::channel(false);
    midi.done = done;
    drop(completed);
    assert!(midi.shutdown().await.is_err());
}

async fn select(midi: &Midi, node: &str, action: &str) {
    midi.command("midi_learn", &serde_json::json!({"assignment":{"target":{"graphId":"main","nodeId":node},"action":action,"graphName":"Programme","nodeName":"Microphone"}})).await.unwrap();
}
async fn receive(midi: &Midi, number: u8) -> serde_json::Value {
    lock(&midi.state).capture.offer(9, &[0xb0, number, 100]);
    midi.command("midi_status", &serde_json::Value::Null)
        .await
        .unwrap()
}

#[tokio::test]
async fn map_mode_is_explicit_and_capture_replaces_only_the_pending_mapping() {
    let (_directory, midi) = fixture();
    assert!(
        midi.command("midi_learn", &serde_json::Value::Null)
            .await
            .is_err()
    );
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    lock(&midi.state).ports.push((
        9,
        Device {
            id: "uid:9".into(),
            name: "Controller".into(),
        },
    ));
    select(&midi, "gain", "volume").await;
    assert_eq!(
        receive(&midi, 7).await["bindings"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        receive(&midi, 8).await["bindings"][0]["control"]["number"],
        8
    );
    select(&midi, "other", "volume").await;
    let status = receive(&midi, 8).await;
    assert_eq!(status["bindings"].as_array().unwrap().len(), 2);
    assert_eq!(status["mapMode"], true);
    assert!(lock(&midi.state).capture.active());
    let first = status["bindings"][0].clone();
    let mut changed = first.clone();
    changed["mode"] = serde_json::json!("offset64");
    midi.command(
        "midi_save",
        &serde_json::json!({"identity":first,"binding":changed}),
    )
    .await
    .unwrap();
    select(&midi, "gain", "volume").await;
    assert_eq!(receive(&midi, 8).await["bindings"][0]["mode"], "offset64");
    midi.command("midi_remove", &serde_json::json!({"binding":changed}))
        .await
        .unwrap();
    assert_eq!(lock(&midi.state).store.bindings().len(), 1);
    midi.command("midi_cancel", &serde_json::Value::Null)
        .await
        .unwrap();
    assert!(!lock(&midi.state).capture.active());
    assert!(!lock(&midi.state).map_mode);
    assert!(
        midi.command("unknown", &serde_json::Value::Null)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn remembered_names_refresh_and_survive_node_and_graph_deletion() {
    let (_directory, midi) = fixture();
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    lock(&midi.state).ports.push((
        9,
        Device {
            id: "uid:9".into(),
            name: "Controller".into(),
        },
    ));
    select(&midi, "gain", "volume").await;
    receive(&midi, 7).await;
    let mut state = lock(&midi.state);
    let snapshot = serde_json::json!({"graphs":[{"id":"main","name":"Renamed graph","nodes":[{"nodeId":"gain","label":"Renamed microphone"}]}]});
    state.refresh_names(&snapshot).unwrap();
    state
        .refresh_names(&serde_json::json!({"graphs":[]}))
        .unwrap();
    assert_eq!(state.store.bindings()[0].graph_name, "Renamed graph");
    assert_eq!(state.store.bindings()[0].node_name, "Renamed microphone");
}

#[tokio::test]
async fn closing_the_editor_clears_map_mode_and_all_queued_input() {
    let (_directory, midi) = fixture();
    let mailbox = Arc::new(mixrelay_midi_control::mailbox::Mailbox::default());
    lock(&midi.state).mailboxes.push(Arc::clone(&mailbox));
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    select(&midi, "gain", "mute").await;
    mailbox.receive(&[0x90, 60, 100]);
    midi.command("midi_cancel", &serde_json::Value::Null)
        .await
        .unwrap();
    assert!(mailbox.take_presses().next().is_none());
    assert!(!lock(&midi.state).capture.active());
}

#[tokio::test]
async fn cancelling_map_mode_rejects_a_callback_that_has_not_published_yet() {
    let (_directory, midi) = fixture();
    let mailbox = Arc::new(mixrelay_midi_control::mailbox::Mailbox::default());
    lock(&midi.state).mailboxes.push(Arc::clone(&mailbox));
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    select(&midi, "gain", "mute").await;
    let capture = Arc::clone(&lock(&midi.state).capture);
    let (paused, pause) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let callback = std::thread::spawn({
        let mailbox = Arc::clone(&mailbox);
        let capture = Arc::clone(&capture);
        move || {
            native::receive(9, &[0x90, 60, 100], &mailbox, &capture, || {
                paused.send(()).unwrap();
                resumed.recv().unwrap();
            });
        }
    });
    pause.await.unwrap();
    midi.command("midi_cancel", &serde_json::Value::Null)
        .await
        .unwrap();
    resume.send(()).unwrap();
    callback.join().unwrap();
    assert!(!lock(&midi.state).map_mode);
    assert_eq!(
        mailbox.take_presses().count(),
        0,
        "old callback survived midi_cancel"
    );
    native::receive(9, &[0x90, 61, 100], &mailbox, &capture, || {});
    assert_eq!(
        mailbox
            .take_presses()
            .map(|input| input.number)
            .collect::<Vec<_>>(),
        [61]
    );
}

#[tokio::test]
async fn graph_names_refresh_even_when_the_node_uses_an_unnamed_runtime_default() {
    let (_directory, midi) = fixture();
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    lock(&midi.state).ports.push((
        9,
        Device {
            id: "uid:9".into(),
            name: "Controller".into(),
        },
    ));
    select(&midi, "gain", "volume").await;
    receive(&midi, 7).await;
    let mut state = lock(&midi.state);
    state.refresh_names(&serde_json::json!({"graphs":[{"id":"main","name":"Renamed graph","nodes":[{"nodeId":"gain","label":null}]}]})).unwrap();
    assert_eq!(state.store.bindings()[0].graph_name, "Renamed graph");
    assert_eq!(state.store.bindings()[0].node_name, "Microphone");
}

#[tokio::test]
async fn an_ignored_duplicate_never_becomes_a_replacement_candidate_and_edits_follow_selection() {
    let (_directory, midi) = fixture();
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    lock(&midi.state).ports.push((
        9,
        Device {
            id: "uid:9".into(),
            name: "Controller".into(),
        },
    ));
    select(&midi, "recorder", "record").await;
    let first = receive(&midi, 7).await["bindings"][0].clone();
    select(&midi, "recorder", "record").await;
    receive(&midi, 7).await;
    let bindings = receive(&midi, 8).await["bindings"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0], first);
    let mut changed = bindings[1].clone();
    changed["action"] = serde_json::json!("stop");
    midi.command(
        "midi_save",
        &serde_json::json!({"identity":bindings[1],"binding":changed}),
    )
    .await
    .unwrap();
    let bindings = receive(&midi, 9).await["bindings"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[1]["action"], "stop");
    assert_eq!(bindings[1]["control"]["number"], 9);
}

#[tokio::test]
async fn learning_cannot_replace_unreadable_preferences() {
    let (repository, midi) = fixture();
    *lock(&repository.0) = Some(b"unreadable original bindings".to_vec());
    *lock(&midi.state) = State::with_repository(repository.clone());
    midi.command("midi_map", &serde_json::json!({"enabled":true}))
        .await
        .unwrap();
    lock(&midi.state).ports.push((
        9,
        Device {
            id: "uid:9".into(),
            name: "Controller".into(),
        },
    ));
    let result = midi.command("midi_learn", &serde_json::json!({"assignment":{"target":{"graphId":"main","nodeId":"gain"},"action":"volume","graphName":"Programme","nodeName":"Microphone"}})).await;
    assert!(matches!(result, Err(MidiError::Preferences)));
    // A previously armed capture is protected at the durable write boundary too.
    {
        let mut state = lock(&midi.state);
        state.selection = Some(Selection { assignment: serde_json::from_value(serde_json::json!({"target":{"graphId":"main","nodeId":"gain"},"action":"volume","graphName":"Programme","nodeName":"Microphone"})).unwrap(), pending: None });
        state.capture.arm();
        state.capture.offer(9, &[0xb0, 7, 100]);
        assert!(matches!(state.observe_learn(), Err(MidiError::Preferences)));
        assert!(state.store.bindings().is_empty());
    }
    assert_eq!(
        lock(&repository.0).as_deref(),
        Some(b"unreadable original bindings".as_slice())
    );
}

impl Midi {
    pub(crate) fn stop_for_test(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub(crate) fn admission(&self) -> super::admission::Admission<'_> {
        super::admission::Admission::new(
            &self.state,
            &self.stop,
            &self.write_gate,
            lock(&self.state).store.generation(),
        )
    }
}
