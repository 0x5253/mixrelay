use super::*;
use mixrelay_midi_control::{Action, Target as MidiTarget};
use mixrelay_midi_control::{Control, Device, Kind, Mode};
use serde_json::json;

fn binding(action: Action) -> Binding {
    Binding {
        control: Control {
            device: Device {
                id: "id".into(),
                name: "controller".into(),
            },
            channel: 0,
            kind: Kind::Cc,
            number: 7,
        },
        target: MidiTarget {
            graph_id: "graph".into(),
            node_id: "gain".into(),
        },
        action,
        mode: Mode::Absolute,
        step_db: 1.0,
        graph_name: "Programme".into(),
        node_name: "Microphone".into(),
        min_db: -60.0,
        max_db: 12.0,
    }
}

fn snapshot() -> Value {
    json!({"session":"same-local-controller", "revision":7,"activeRevision":7,
    "enginePower":{"enabled":true,"transitioning":false},
    "graphs":[{"id":"graph","nodes":[{"nodeId":"gain","control":{"enabled":true,"muted":false},
    "capabilities":{"power":true,"mute":true,"recorder":true},"gainDb":0,
    "recorder":{"state":"paused","path":"take.wav"}}]}]})
}

#[tokio::test]
async fn midi_and_stream_deck_reach_the_same_in_process_node_control_boundary() {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
    let fixture = mixrelay_test_support::FixtureDir::ipc("midi").unwrap();
    let path = fixture.join("engine.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let mut snapshot = snapshot();
    snapshot.as_object_mut().unwrap().remove("session");
    snapshot["structureGeneration"] = json!(1);
    snapshot["graphs"][0]["name"] = json!("Programme");
    snapshot["graphs"][0]["nodes"][0]["kind"] = json!("gain");
    snapshot["graphs"][0]["nodes"][0]["label"] = json!(null);
    snapshot["graphs"][0]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("recorder");
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..3 {
            let (stream, _) = listener.accept().await.unwrap();
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
            let response = if route.starts_with("GET ") {
                snapshot.clone()
            } else {
                json!({"revision":8})
            };
            requests.push((route, body));
            let response = response.to_string();
            reader.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    let control = LocalControl::new(
        crate::control_proxy::ControlProxyClient::with_socket(Some(path)),
        #[cfg(target_os = "macos")]
        std::sync::Arc::default(),
    )
    .unwrap();
    let (_directory, midi) = crate::midi::tests::fixture();
    control
        .midi_action(&binding(Action::Mute), None, &midi.admission())
        .await;
    let request = serde_json::from_value(json!({"id":1,"command":{"type":"node","session":control.session,"target":{"graphId":"graph","nodeId":"gain"},"request":{"expectedRevision":7,"muted":true}}})).unwrap();
    let reply = control.dispatch(request).await;
    assert_eq!(reply["status"], 200);
    let requests = tokio::time::timeout(std::time::Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(requests[1], requests[2]);
    assert_eq!(
        requests[1].0.trim(),
        "PUT /api/v1/graphs/graph/nodes/gain/control HTTP/1.1"
    );
    assert_eq!(requests[1].1, json!({"expectedRevision":7,"muted":true}));
}

mod dispatch_regressions;
