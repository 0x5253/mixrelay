use super::*;
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    sync::{mpsc, oneshot},
};

struct Call {
    route: String,
    body: Value,
    reply: oneshot::Sender<Value>,
}

struct Engine {
    _fixture: mixrelay_test_support::FixtureDir,
    control: Arc<LocalControl>,
    calls: mpsc::UnboundedReceiver<Call>,
    server: tokio::task::JoinHandle<()>,
}
impl Engine {
    fn new() -> Self {
        let fixture = mixrelay_test_support::FixtureDir::ipc("midi").unwrap();
        let path = fixture.join("engine.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let (send, calls) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let mut reader = BufReader::new(stream);
                let mut route = String::new();
                reader.read_line(&mut route).await.unwrap();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).await.unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).await.unwrap();
                let body = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                let (reply, response) = oneshot::channel();
                if send.send(Call { route, body, reply }).is_err() {
                    break;
                }
                let response = response.await.unwrap().to_string();
                let _ = reader.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await;
            }
        });
        let control = Arc::new(
            LocalControl::new(
                crate::control_proxy::ControlProxyClient::with_socket(Some(path)),
                #[cfg(target_os = "macos")]
                Arc::default(),
            )
            .unwrap(),
        );
        Self {
            _fixture: fixture,
            control,
            calls,
            server,
        }
    }
    async fn next(&mut self) -> Call {
        tokio::time::timeout(std::time::Duration::from_secs(3), self.calls.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn engine_snapshot(revision: u64, active: u64) -> Value {
    let mut value = snapshot();
    value.as_object_mut().unwrap().remove("session");
    value["revision"] = json!(revision);
    value["activeRevision"] = json!(active);
    value["structureGeneration"] = json!(1);
    value["graphs"][0]["name"] = json!("Programme");
    let node = &mut value["graphs"][0]["nodes"][0];
    node["kind"] = json!("gain");
    node["label"] = Value::Null;
    node.as_object_mut().unwrap().remove("recorder");
    let mut other = node.clone();
    other["nodeId"] = json!("other");
    value["graphs"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(other);
    value
}

#[tokio::test]
async fn pending_application_is_acknowledged_before_the_next_fan_out_target() {
    let mut engine = Engine::new();
    let (_directory, midi) = crate::midi::tests::fixture();
    let control = Arc::clone(&engine.control);
    let mut dispatch = tokio::spawn(async move {
        let admission = midi.admission();
        let mut results = Vec::new();
        for node in ["gain", "other"] {
            let mut mapping = binding(Action::Mute);
            mapping.target.node_id = node.into();
            results.push(control.midi_action(&mapping, None, &admission).await);
        }
        results
    });
    let mut writes = Vec::new();
    let mut pending_reads = 0;
    let results = loop {
        tokio::select! {
            result = &mut dispatch => break result.unwrap(),
            call = engine.next() => {
                let response = if call.route.starts_with("GET ") {
                    if writes.is_empty() { engine_snapshot(7,7) }
                    else { pending_reads += 1; engine_snapshot(8, if pending_reads == 1 { 7 } else { 8 }) }
                } else {
                    writes.push(call.body);
                    json!({"protocolVersion":1,"revision":8,"state":if writes.len() == 1 {"pending"} else {"active"},"requiresGraphReload":false})
                };
                call.reply.send(response).unwrap();
            }
        }
    };
    assert_eq!(
        results,
        [true, true],
        "a successful Pending write must not drop the next target"
    );
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[1]["expectedRevision"], 8);
}

#[tokio::test]
async fn mapping_changes_during_snapshot_io_fence_mute_recording_and_gain_writes() {
    for action in [Action::Mute, Action::Record, Action::Volume] {
        for command in ["midi_map", "midi_remove"] {
            let mut engine = Engine::new();
            let (_directory, midi) = crate::midi::tests::fixture();
            let midi = Arc::new(midi);
            let task_midi = Arc::clone(&midi);
            let control = Arc::clone(&engine.control);
            let mapping = binding(action);
            let identity = serde_json::to_value(&mapping).unwrap();
            let dispatch = tokio::spawn(async move {
                control
                    .midi_action(
                        &mapping,
                        (action == Action::Volume).then_some((-3.0, Some(0.0))),
                        &task_midi.admission(),
                    )
                    .await
            });
            let snapshot_read = engine.next().await;
            assert!(snapshot_read.route.starts_with("GET "));
            midi.command(command, &json!({"enabled":true,"binding":identity}))
                .await
                .unwrap();
            let mut value = engine_snapshot(7, 7);
            if action == Action::Record {
                value["graphs"][0]["nodes"][0]["recorder"] =
                    json!({"state":"paused","path":"take.wav","frames":0,"error":null});
            }
            snapshot_read.reply.send(value).unwrap();
            tokio::pin!(dispatch);
            tokio::select! {
                result = &mut dispatch => assert!(!result.unwrap()),
                call = engine.next() => panic!("obsolete {action:?} reached write boundary: {}", call.route),
            }
        }
    }
}

#[tokio::test]
async fn accepted_pending_waits_yield_to_mapping_changes_and_shutdown() {
    for stopping in [false, true] {
        let mut engine = Engine::new();
        let (_directory, midi) = crate::midi::tests::fixture();
        let midi = Arc::new(midi);
        let task_midi = Arc::clone(&midi);
        let control = Arc::clone(&engine.control);
        let dispatch = tokio::spawn(async move {
            control
                .midi_action(&binding(Action::Mute), None, &task_midi.admission())
                .await
        });
        engine
            .next()
            .await
            .reply
            .send(engine_snapshot(7, 7))
            .unwrap();
        let write = engine.next().await;
        assert!(write.route.starts_with("PUT "));
        write
            .reply
            .send(json!({"revision":8,"state":"pending"}))
            .unwrap();
        // Hold the acknowledgement snapshot: cancellation must not wait for engine IO.
        let pending_snapshot = engine.next().await;
        assert!(pending_snapshot.route.starts_with("GET "));
        if stopping {
            midi.stop_for_test();
        } else {
            midi.command("midi_map", &json!({"enabled":true}))
                .await
                .unwrap();
        }
        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(3), dispatch)
                .await
                .unwrap()
                .unwrap()
        );
        drop(pending_snapshot);
    }
}

#[tokio::test]
async fn mapping_changes_wait_for_already_admitted_writes_then_fence_the_next_write() {
    let mut engine = Engine::new();
    let (_directory, midi) = crate::midi::tests::fixture();
    let midi = Arc::new(midi);
    let task_midi = Arc::clone(&midi);
    let control = Arc::clone(&engine.control);
    let dispatch = tokio::spawn(async move {
        let admission = task_midi.admission();
        let first = control
            .midi_action(&binding(Action::Mute), None, &admission)
            .await;
        let second = control
            .midi_action(&binding(Action::Mute), None, &admission)
            .await;
        [first, second]
    });
    engine
        .next()
        .await
        .reply
        .send(engine_snapshot(7, 7))
        .unwrap();
    let write = engine.next().await;
    let arguments = json!({"enabled":true});
    let changing = midi.command("midi_map", &arguments);
    tokio::pin!(changing);
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(changing.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    write
        .reply
        .send(json!({"revision":8,"state":"active"}))
        .unwrap();
    changing.await.unwrap();
    assert_eq!(dispatch.await.unwrap(), [true, false]);
}
