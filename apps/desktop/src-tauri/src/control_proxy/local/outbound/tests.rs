use super::*;
use mixrelay_control_protocol::TelemetryDecoder;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt as _, BufReader};

fn window(sequence: u64, peak: f32) -> ControlTelemetry {
    ControlTelemetry {
        engine_incarnation: "engine".into(),
        sequence,
        meters: mixrelay_control_protocol::MeterSnapshot {
            protocol_version: 1,
            revision: 1,
            nodes: vec![mixrelay_control_protocol::NodeMeterSnapshot {
                node_id: "n".into(),
                port_id: "out".into(),
                peaks: vec![peak],
                dynamics: None,
            }],
        },
        state: None,
    }
}

fn frame(next: Next) -> Value {
    match next {
        Next::Frame(frame) => frame,
        Next::Idle => panic!("idle"),
        Next::Done => panic!("done"),
    }
}

#[test]
fn replies_leave_first_in_order_and_each_stream_keeps_its_folded_latest_window() {
    let outbound = Outbound::default();
    outbound.open(7);
    outbound.window(7, window(1, 0.9));
    outbound.window(7, window(2, 0.1));
    outbound.reply(json!({"id": 1, "status": 102}));
    outbound.reply(json!({"id": 1, "status": 200}));
    assert_eq!(frame(outbound.next())["status"], 102);
    assert_eq!(frame(outbound.next())["status"], 200);
    let pushed = frame(outbound.next());
    assert_eq!(
        (pushed["id"].clone(), pushed["status"].clone()),
        (7.into(), 206.into())
    );
    let decoded = TelemetryDecoder::default()
        .json(pushed["body"].clone())
        .unwrap();
    // The newest sequence carries the interval maximum.
    assert_eq!(decoded.sequence, 2);
    assert_eq!(decoded.meters.nodes[0].peaks, [0.9]);
    assert!(matches!(outbound.next(), Next::Idle));
    // Later windows of the same stream omit the bindings it already carried.
    outbound.window(7, window(3, 0.2));
    assert!(
        frame(outbound.next())["body"]["meters"]
            .get("bindings")
            .is_none()
    );
}

#[test]
fn busy_streams_take_turns_and_end_after_their_pending_window() {
    let outbound = Outbound::default();
    for id in [3, 5] {
        outbound.open(id);
    }
    // Windows for unopened or cancelled streams are dropped.
    outbound.window(4, window(1, 0.5));
    outbound.end(4, "unknown");
    for round in 0..2 {
        outbound.window(3, window(round, 0.5));
        outbound.window(5, window(round, 0.5));
    }
    let order: Vec<_> = (0..2)
        .map(|_| frame(outbound.next())["id"].clone())
        .collect();
    assert_eq!(order, [3, 5]);
    outbound.window(3, window(9, 0.5));
    outbound.end(3, "engine stopped");
    outbound.end(3, "ignored");
    assert_eq!(frame(outbound.next())["status"], 206);
    let ended = frame(outbound.next());
    assert_eq!(
        (ended["id"].clone(), ended["status"].clone()),
        (3.into(), 503.into())
    );
    assert_eq!(ended["body"]["parameters"]["reason"], "engine stopped");
    outbound.window(3, window(10, 0.5));
    outbound.window(5, window(10, 0.5));
    outbound.cancel(5);
    assert!(matches!(outbound.next(), Next::Idle));
    outbound.finish();
    assert!(matches!(outbound.next(), Next::Done));
}

#[test]
fn an_oversized_reply_fails_only_its_own_request() {
    let large = "x".repeat(RESPONSE_LIMIT);
    let bytes = encode(&json!({"id": 4, "status": 200, "body": {"ok": large}})).unwrap();
    let error: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        (error["id"].clone(), error["status"].clone()),
        (4.into(), 500.into())
    );
    assert_eq!(bytes.last(), Some(&b'\n'));
}

#[tokio::test]
async fn the_writer_drains_queued_frames_when_finishing_then_rejects_more() {
    let outbound = Arc::new(Outbound::default());
    let (runtime, editor) = tokio::io::duplex(1 << 16);
    let writer = tokio::spawn({
        let outbound = Arc::clone(&outbound);
        let mut runtime = runtime;
        async move {
            outbound
                .write_all(&mut runtime, Duration::from_secs(1))
                .await
        }
    });
    outbound.reply(json!({"id": 1, "status": 200, "body": {}}));
    let mut editor = BufReader::new(editor);
    let mut line = String::new();
    editor.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"id\":1"));
    outbound.reply(json!({"id": 2, "status": 200, "body": {}}));
    outbound.finish();
    writer.await.unwrap().unwrap();
    line.clear();
    editor.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"id\":2"));
    // A finished writer closed the queue: nothing is accepted and `closed` resolves.
    outbound.open(9);
    outbound.reply(json!({"id": 3}));
    assert!(matches!(outbound.next(), Next::Done));
    outbound.closed().await;
}

#[tokio::test]
async fn a_stalled_or_failed_write_closes_the_queue() {
    // A one-byte pipe the editor never drains stalls the first frame.
    let outbound = Arc::new(Outbound::default());
    let (mut runtime, _editor) = tokio::io::duplex(1);
    outbound.reply(json!({"id": 1, "status": 200, "body": {}}));
    let stalled = outbound
        .write_all(&mut runtime, Duration::from_millis(20))
        .await
        .unwrap_err();
    assert_eq!(stalled.kind(), io::ErrorKind::TimedOut);
    outbound.closed().await;
    // A peer that left fails the write itself.
    let outbound = Outbound::default();
    let (mut runtime, editor) = tokio::io::duplex(64);
    drop(editor);
    outbound.reply(json!({"id": 1, "status": 200, "body": {}}));
    assert!(
        outbound
            .write_all(&mut runtime, Duration::from_secs(1))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn repeated_pending_frames_coalesce_and_the_backlog_counts_unwritten_replies() {
    let outbound = Arc::new(Outbound::default());
    for _ in 0..3 {
        outbound.reply(json!({"id": 1, "status": 102, "body": {}}));
    }
    outbound.reply(json!({"id": 2, "status": 102, "body": {}}));
    outbound.reply(json!({"id": 1, "status": 200, "body": {}}));
    assert_eq!(outbound.backlog(), 3);
    let (mut runtime, editor) = tokio::io::duplex(1 << 16);
    // Already below the limit: resolves without a write.
    outbound.backlog_below(4).await;
    let drained = tokio::spawn({
        let outbound = Arc::clone(&outbound);
        async move { outbound.backlog_below(1).await }
    });
    tokio::task::yield_now().await;
    assert!(!drained.is_finished());
    let writer = tokio::spawn({
        let outbound = Arc::clone(&outbound);
        async move {
            outbound
                .write_all(&mut runtime, Duration::from_secs(1))
                .await
        }
    });
    drained.await.unwrap();
    let mut editor = BufReader::new(editor);
    for _ in 0..3 {
        let mut line = String::new();
        editor.read_line(&mut line).await.unwrap();
    }
    assert_eq!(outbound.backlog(), 0);
    outbound.reply(json!({"id": 3, "status": 102, "body": {}}));
    outbound.close();
    // A closed queue no longer waits for writes.
    outbound.backlog_below(0).await;
    writer.await.unwrap().unwrap();
}
