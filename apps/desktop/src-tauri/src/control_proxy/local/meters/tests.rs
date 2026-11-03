use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::mpsc;

type Chunk = Result<Option<Vec<u8>>, String>;
/// A prepared engine stream: its body, a refusal, or (`None`) an open that never completes.
type Prepared = Option<Result<mpsc::UnboundedReceiver<Chunk>, String>>;

/// An engine whose streams are fed by the test; each open takes the next prepared stream.
#[derive(Default)]
struct Engine {
    streams: Mutex<Vec<Prepared>>,
    opened: AtomicUsize,
}

impl Engine {
    fn stream(&self) -> mpsc::UnboundedSender<Chunk> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.streams.lock().unwrap().insert(0, Some(Ok(receiver)));
        sender
    }

    fn refuse(&self, reason: &str) {
        self.streams
            .lock()
            .unwrap()
            .insert(0, Some(Err(reason.to_owned())));
    }

    /// The next open never completes.
    fn hang(&self) {
        self.streams.lock().unwrap().insert(0, None);
    }
}

struct Body(mpsc::UnboundedReceiver<Chunk>);

impl TelemetrySource for Engine {
    type Chunks = Body;

    fn open(&self) -> impl Future<Output = Result<Body, String>> + Send {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let next = self
            .streams
            .lock()
            .unwrap()
            .pop()
            .expect("a prepared stream");
        async move {
            match next {
                Some(stream) => stream.map(Body),
                None => std::future::pending().await,
            }
        }
    }
}

impl TelemetryChunks for Body {
    async fn next_chunk(&mut self) -> Chunk {
        self.0.recv().await.unwrap_or(Ok(None))
    }
}

/// One engine window, as the engine encodes it at the start of a stream.
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

/// One engine window as the bytes of a stream chunk.
fn event(sequence: u64, peak: f32) -> Vec<u8> {
    let window =
        mixrelay_control_protocol::TelemetryEncoder::default().json(&window(sequence, peak));
    format!("event: telemetry\ndata: {window}\n\n").into_bytes()
}

fn hub(engine: &Arc<Engine>) -> (Arc<TelemetryHub<Engine>>, watch::Sender<bool>) {
    let (stop, stopped) = watch::channel(false);
    (TelemetryHub::new(Arc::clone(engine), stopped), stop)
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

fn state(generation: u64) -> mixrelay_control_protocol::ControlStateSnapshot {
    mixrelay_control_protocol::ControlStateSnapshot {
        generation,
        status: mixrelay_control_protocol::AppStatus::default(),
        controls: None,
        editors: vec![],
        graph: mixrelay_control_protocol::GraphRuntimeStatus {
            pending_reason: None,
            protocol_version: 1,
            active_revision: 1,
            submitted_revision: 1,
        },
    }
}

#[tokio::test]
async fn latest_state_survives_backpressure_and_is_replayed_to_a_late_editor() {
    let engine = Arc::new(Engine::default());
    let feed = engine.stream();
    let (hub, stop) = hub(&engine);
    let slow = hub.subscribe();
    let mut packet = window(1, 0.0);
    packet.meters.nodes.clear();
    for generation in 1..=4 {
        packet.sequence = generation;
        packet.state = Some(state(generation));
        feed.send(Ok(Some(
            format!("data: {}\n\n", serde_json::to_string(&packet).unwrap()).into_bytes(),
        )))
        .unwrap();
    }
    packet.sequence = 5;
    packet.state = None;
    feed.send(Ok(Some(
        format!("data: {}\n\n", serde_json::to_string(&packet).unwrap()).into_bytes(),
    )))
    .unwrap();
    settle().await;
    let latest = slow.take().await.unwrap();
    assert_eq!(latest.sequence, 5);
    assert_eq!(latest.state.unwrap().generation, 4);
    let late = hub.subscribe();
    assert_eq!(late.take().await.unwrap().state.unwrap().generation, 4);
    assert_eq!(engine.opened.load(Ordering::SeqCst), 1);
    stop.send(true).unwrap();
    assert!(late.take().await.is_err());
}

#[tokio::test]
async fn editors_share_one_engine_stream_and_a_slow_one_keeps_the_interval_maximum() {
    let engine = Arc::new(Engine::default());
    let feed = engine.stream();
    let (hub, _stop) = hub(&engine);
    let fast = hub.subscribe();
    let slow = hub.subscribe();
    feed.send(Ok(Some(event(1, 0.9)))).unwrap();
    assert_eq!(fast.take().await.unwrap().sequence, 1);
    feed.send(Ok(Some(b": keep-alive\n\n".to_vec()))).unwrap();
    feed.send(Ok(Some(event(2, 0.1)))).unwrap();
    assert_eq!(fast.take().await.unwrap().sequence, 2);
    let merged = slow.take().await.unwrap();
    assert_eq!((merged.sequence, merged.meters.nodes[0].peaks[0]), (2, 0.9));
    assert_eq!(engine.opened.load(Ordering::SeqCst), 1);
    drop(feed);
    assert_eq!(fast.take().await.unwrap_err(), "telemetry stream ended");
    assert_eq!(slow.take().await.unwrap_err(), "telemetry stream ended");
}

#[tokio::test]
async fn the_stream_closes_with_its_last_reader_and_reopens_for_the_next() {
    let engine = Arc::new(Engine::default());
    let first = engine.stream();
    let (hub, _stop) = hub(&engine);
    drop(hub.subscribe());
    first.send(Ok(Some(event(1, 0.5)))).unwrap();
    settle().await;
    assert!(
        first.is_closed(),
        "the engine stream closes without readers"
    );
    let second = engine.stream();
    let reader = hub.subscribe();
    second.send(Ok(Some(event(2, 0.5)))).unwrap();
    assert_eq!(reader.take().await.unwrap().sequence, 2);
    assert_eq!(engine.opened.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_retired_upstream_never_ends_its_successors_readers() {
    let engine = Arc::new(Engine::default());
    let (hub, _stop) = hub(&engine);
    // Generation 1 is retired; a reader of generation 2 must survive its cleanup.
    hub.readers().upstream = Some(2);
    let reader = Arc::new(Mailbox::default());
    hub.readers().mailboxes.push(Arc::downgrade(&reader));
    engine.refuse("old engine");
    Arc::clone(&hub).upstream(1).await;
    assert_eq!(hub.readers().upstream, Some(2));
    assert_eq!(hub.readers().mailboxes.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn readers_learn_why_the_engine_stream_failed() {
    let engine = Arc::new(Engine::default());
    let (hub, stop) = hub(&engine);
    engine.refuse("meter telemetry HTTP 404");
    assert_eq!(
        hub.subscribe().take().await.unwrap_err(),
        "meter telemetry HTTP 404"
    );
    engine.hang();
    assert_eq!(
        hub.subscribe().take().await.unwrap_err(),
        "telemetry stream did not open"
    );
    let silent = engine.stream();
    assert_eq!(
        hub.subscribe().take().await.unwrap_err(),
        "telemetry stream stalled"
    );
    drop(silent);
    let failing = engine.stream();
    failing.send(Err("connection reset".to_owned())).unwrap();
    assert_eq!(
        hub.subscribe().take().await.unwrap_err(),
        "connection reset"
    );
    let oversized = engine.stream();
    oversized
        .send(Ok(Some(vec![b'a'; 4 * 1024 * 1024 + 1])))
        .unwrap();
    assert_eq!(
        hub.subscribe().take().await.unwrap_err(),
        "telemetry event exceeds its limit"
    );
    let _open = engine.stream();
    let reader = hub.subscribe();
    stop.send(true).unwrap();
    assert_eq!(reader.take().await.unwrap_err(), "runtime stopped");
}

async fn frame(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

/// A connection's writer over an in-memory pipe, and the editor's end of it.
fn connection() -> (
    Arc<super::super::outbound::Outbound>,
    BufReader<tokio::io::DuplexStream>,
) {
    let outbound = Arc::new(super::super::outbound::Outbound::default());
    let (mut runtime, editor) = tokio::io::duplex(1 << 16);
    tokio::spawn({
        let outbound = Arc::clone(&outbound);
        async move {
            outbound
                .write_all(&mut runtime, Duration::from_secs(1))
                .await
        }
    });
    (outbound, BufReader::new(editor))
}

#[tokio::test]
async fn a_pump_writes_windows_then_the_terminal_frame() {
    let engine = Arc::new(Engine::default());
    let feed = engine.stream();
    let (hub, _stop) = hub(&engine);
    let (outbound, mut editor) = connection();
    outbound.open(7);
    let pump = tokio::spawn(pump(7, Arc::clone(&hub), Arc::clone(&outbound)));
    feed.send(Ok(Some(event(1, 0.5)))).unwrap();
    let window = frame(&mut editor).await;
    assert_eq!(
        (window["id"].clone(), window["status"].clone()),
        (7.into(), 206.into())
    );
    assert_eq!(window["body"]["sequence"], 1);
    drop(feed);
    let ended = frame(&mut editor).await;
    assert_eq!(ended["status"], 503);
    assert_eq!(
        ended["body"]["parameters"]["reason"],
        "telemetry stream ended"
    );
    pump.await.unwrap();
}

#[tokio::test]
async fn an_engine_stream_that_loses_its_bindings_ends_instead_of_misplacing_meters() {
    let engine = Arc::new(Engine::default());
    let feed = engine.stream();
    let (hub, _stop) = hub(&engine);
    let reader = hub.subscribe();
    let mut encoder = mixrelay_control_protocol::TelemetryEncoder::default();
    let _ = encoder.json(&window(1, 0.5));
    let omitted = encoder.json(&window(2, 0.5));
    feed.send(Ok(Some(format!("data: {omitted}\n\n").into_bytes())))
        .unwrap();
    assert_eq!(reader.take().await.unwrap_err(), "meter bindings missing");
    let second = engine.stream();
    let reader = hub.subscribe();
    let _ = second.send(Ok(Some(b"data: {\n\n".to_vec())));
    assert!(reader.take().await.is_err());
}
