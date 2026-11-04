use super::super::forward_transport_error;
use super::*;
use std::{fs, time::Instant};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;

#[tokio::test]
async fn an_interrupted_initial_catalog_read_reports_retryable_transport_without_replaying() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let frame = request(&mut reader).await;
        assert_eq!(frame["command"]["name"], "control_get_graph_catalog");
    });
    let failure = connection
        .exchange("control_get_graph_catalog", Value::Null)
        .await
        .unwrap_err();
    let result = forward_transport_error("control_get_graph_catalog", &failure).unwrap();
    assert_eq!(result["status"], 503);
    assert_eq!(result["body"]["code"], "control_unavailable");
    assert_eq!(
        result["body"]["parameters"]["diagnostic"]["boundary"],
        "editor_runtime"
    );
    assert_eq!(
        result["body"]["parameters"]["diagnostic"]["stage"],
        "response"
    );
    assert_eq!(
        result["body"]["parameters"]["diagnostic"]["delivery"],
        "uncertain"
    );
    server.await.unwrap();
}

#[test]
fn catalog_transport_classification_keeps_security_protocol_and_mutation_failures_terminal() {
    for failure in [
        ConnectionFailure::PeerMismatch,
        ConnectionFailure::InvalidResponse,
        ConnectionFailure::RequestTooLarge,
    ] {
        let reason = failure.to_string();
        assert_eq!(
            forward_transport_error("control_get_graph_catalog", &failure),
            Err(Value::String(reason))
        );
    }
    assert!(
        forward_transport_error(
            "control_mutate_graph_catalog",
            &ConnectionFailure::ResponseTimeout
        )
        .is_err()
    );
}

fn fixture_connection(directory: &std::path::Path) -> Connection {
    Connection::at_socket(
        directory.join("service.sock"),
        fs::metadata(directory).unwrap().uid(),
        Arc::new(mixrelay_time::ManualClock::default()),
    )
}

#[tokio::test]
async fn a_missing_socket_retains_its_os_error_and_unsent_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let failure = connection
        .exchange("control_get_graph_catalog", Value::Null)
        .await
        .unwrap_err();
    assert!(matches!(failure, ConnectionFailure::Connect(_)));
    let response = forward_transport_error("control_get_graph_catalog", &failure).unwrap();
    let diagnostic = &response["body"]["parameters"]["diagnostic"];
    assert_eq!(diagnostic["platform"], std::env::consts::OS);
    assert_eq!(diagnostic["boundary"], "editor_runtime");
    assert_eq!(diagnostic["stage"], "connect");
    assert_eq!(diagnostic["delivery"], "not_sent");
    assert_eq!(diagnostic["osError"], libc::ENOENT);
    assert!(
        !diagnostic
            .to_string()
            .contains(directory.path().to_str().unwrap())
    );
}

async fn request(reader: &mut BufReader<tokio::net::UnixStream>) -> Value {
    let mut frame = String::new();
    reader.read_line(&mut frame).await.unwrap();
    serde_json::from_str(&frame).unwrap()
}

async fn send(reader: &mut BufReader<tokio::net::UnixStream>, frame: &Value) {
    reader
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
}

#[tokio::test]
async fn heartbeat_keeps_one_command_alive_and_returns_the_service_error() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut frame = String::new();
        reader.read_line(&mut frame).await.unwrap();
        let request: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(request["command"]["name"], "driver_mutate");
        for body in [
            json!({"id":request["id"],"status":102,"body":{}}),
            json!({"id":request["id"],"status":200,"body":{"error":{"code":"graph_revision_stale"}}}),
        ] {
            reader
                .get_mut()
                .write_all(format!("{body}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    assert_eq!(
        connection
            .invoke("driver_mutate", json!({}))
            .await
            .unwrap_err(),
        json!({"code":"graph_revision_stale"})
    );
    server.await.unwrap();
}

/// Answers `count` requests on one accepted stream with `{"ok": id}`.
async fn answer(reader: &mut BufReader<tokio::net::UnixStream>, count: usize) {
    for _ in 0..count {
        let mut frame = String::new();
        reader.read_line(&mut frame).await.unwrap();
        let request: Value = serde_json::from_str(&frame).unwrap();
        let reply = json!({"id":request["id"],"status":200,"body":{"ok":request["id"]}});
        reader
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn sequential_requests_reuse_one_open_connection() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        answer(&mut BufReader::new(stream), 2).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    for _ in 0..2 {
        assert!(
            connection
                .invoke("control_get_meters", json!({}))
                .await
                .is_ok()
        );
    }
    server.await.unwrap();
}

#[tokio::test]
async fn a_connection_the_controller_closed_is_replaced_not_reused() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        answer(&mut BufReader::new(stream), 1).await;
        // The first stream is closed here; the next request needs a new connection.
        let (stream, _) = listener.accept().await.unwrap();
        answer(&mut BufReader::new(stream), 1).await;
    });
    assert!(
        connection
            .invoke("control_get_meters", json!({}))
            .await
            .is_ok()
    );
    // Give the close time to reach the client before it chooses a connection.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(connection.invoke("driver_mutate", json!({})).await.is_ok());
    server.await.unwrap();
}

#[tokio::test]
async fn disconnect_after_delivery_never_replays_a_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut frame = String::new();
        reader.read_line(&mut frame).await.unwrap();
        drop(reader);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let error = connection
        .invoke("control_apply_graph", json!({"document":{}}))
        .await
        .unwrap_err();
    assert!(error.as_str().unwrap().contains("not replayed"));
    server.await.unwrap();
}

#[tokio::test]
async fn another_users_socket_is_rejected_before_command_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let mut connection = fixture_connection(directory.path());
    connection.uid = connection.uid.wrapping_add(1);
    let listener = UnixListener::bind(&connection.path).unwrap();
    assert!(
        connection
            .invoke("driver_mutate", json!({"command":{}}))
            .await
            .unwrap_err()
            .as_str()
            .unwrap()
            .contains("different user")
    );
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut byte = [0];
    assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
}

/// A runtime that answers one meter subscription with `replies` (the request id fills in
/// `"id": null`), then closes the connection.
fn meter_runtime(connection: &Connection, replies: Vec<Value>) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(&connection.path).unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let request = request(&mut reader).await;
        assert_eq!(request["command"]["type"], "subscribe_telemetry");
        for mut reply in replies {
            if reply["id"].is_null() {
                reply["id"] = request["id"].clone();
            }
            send(&mut reader, &reply).await;
        }
    })
}

fn meter_window(sequence: u64) -> Value {
    let window = mixrelay_control_protocol::ControlTelemetry {
        engine_incarnation: "engine".into(),
        sequence,
        meters: mixrelay_control_protocol::MeterSnapshot {
            protocol_version: 1,
            revision: 1,
            nodes: vec![],
        },
        state: None,
    };
    let body = mixrelay_control_protocol::TelemetryEncoder::default().json(&window);
    json!({"id": null, "status": 206, "body": body})
}

#[tokio::test]
async fn a_meter_subscription_yields_pushed_windows_until_the_runtime_ends_it() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let server = meter_runtime(
        &connection,
        vec![
            meter_window(1),
            meter_window(2),
            json!({"id": null, "status": 503, "body": {}}),
        ],
    );
    let mut subscription = connection.subscribe_telemetry().await.unwrap();
    let sequence = |window: Option<mixrelay_control_protocol::ControlTelemetry>| {
        window.map(|window| window.sequence)
    };
    // Windows that arrive faster than they are read fold into the latest.
    let mut received = vec![sequence(subscription.next().await)];
    while let Some(window) = sequence(subscription.next().await) {
        received.push(Some(window));
    }
    assert_eq!(received.last(), Some(&Some(2)));
    // The terminal frame ends the stream, as does a closed connection.
    assert_eq!(sequence(subscription.next().await), None);
    server.await.unwrap();
}

#[tokio::test]
async fn a_meter_subscription_without_a_first_window_is_refused() {
    for (replies, reason) in [
        (
            vec![
                json!({"id": null, "status": 503, "body": {"parameters": {"reason": "HTTP 404"}}}),
            ],
            "HTTP 404",
        ),
        (
            vec![json!({"id": null, "status": 400, "body": {}})],
            "meter push unavailable",
        ),
        // A runtime that does not know the command closes the connection.
        (Vec::new(), "runtime connection closed"),
        (
            vec![json!({"id": null, "status": 206, "body": {"sequence": "x"}})],
            "invalid type: string \"x\", expected u64",
        ),
    ] {
        // A fresh connection per case: a shared link may not yet have seen the previous
        // runtime close, and a request sent on it is never replayed.
        let directory = tempfile::tempdir().unwrap();
        let connection = fixture_connection(directory.path());
        let server = meter_runtime(&connection, replies);
        let refused = connection.subscribe_telemetry().await.err().unwrap();
        assert!(refused.to_string().starts_with(reason), "{refused}");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn concurrent_commands_share_one_connection_and_may_be_answered_out_of_order() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let first = request(&mut reader).await;
        let second = request(&mut reader).await;
        // Unknown IDs are ignored; then the later request is answered first.
        send(
            &mut reader,
            &json!({"id": 0, "status": 200, "body": {"ok": 0}}),
        )
        .await;
        send(&mut reader, &json!({"id": "x"})).await;
        for request in [&second, &first] {
            let name = request["command"]["name"].clone();
            send(
                &mut reader,
                &json!({"id": request["id"], "status": 200, "body": {"ok": name}}),
            )
            .await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let (slow, fast) = tokio::join!(
        connection.invoke("slow", json!({})),
        connection.invoke("fast", json!({}))
    );
    assert_eq!(
        (slow.unwrap(), fast.unwrap()),
        (json!("slow"), json!("fast"))
    );
    server.await.unwrap();
}

#[tokio::test]
async fn a_subscription_shares_the_link_with_commands_and_is_cancelled_when_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let listener = UnixListener::bind(&connection.path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let subscribe = request(&mut reader).await;
        let mut window = meter_window(1);
        window["id"] = subscribe["id"].clone();
        send(&mut reader, &window).await;
        let command = request(&mut reader).await;
        send(
            &mut reader,
            &json!({"id": command["id"], "status": 102, "body": {}}),
        )
        .await;
        // Telemetry keeps flowing while the command is pending.
        let mut second = meter_window(2);
        second["id"] = subscribe["id"].clone();
        send(&mut reader, &second).await;
        send(
            &mut reader,
            &json!({"id": command["id"], "status": 200, "body": {"ok": true}}),
        )
        .await;
        let cancel = request(&mut reader).await;
        assert_eq!(cancel["command"]["type"], "cancel");
        assert_eq!(cancel["command"]["subscription"], subscribe["id"]);
        // A connection the editor stopped using is closed by the editor, never reused late.
        let mut byte = [0];
        assert_eq!(reader.read(&mut byte).await.unwrap(), 0);
    });
    let mut subscription = connection.subscribe_telemetry().await.unwrap();
    assert_eq!(
        subscription.next().await.map(|window| window.sequence),
        Some(1)
    );
    assert_eq!(
        connection.invoke("driver_mutate", json!({})).await,
        Ok(json!(true))
    );
    assert_eq!(
        subscription.next().await.map(|window| window.sequence),
        Some(2)
    );
    drop(subscription);
    let link = connection.link.lock().await.take().unwrap();
    tokio::task::yield_now().await;
    link.close();
    server.await.unwrap();
}

#[tokio::test]
async fn a_missing_runtime_socket_fails_every_request_promptly() {
    let directory = tempfile::tempdir().unwrap();
    let connection = fixture_connection(directory.path());
    let started = Instant::now();
    for _ in 0..3 {
        assert!(connection.invoke("hello", Value::Null).await.is_err());
        let hello = connection.invoke("hello", Value::Null).await;
        assert!(
            !hello.is_ok_and(|hello| crate::background::startup::compatibility().accepts(&hello))
        );
    }
    assert!(connection.subscribe_telemetry().await.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}
