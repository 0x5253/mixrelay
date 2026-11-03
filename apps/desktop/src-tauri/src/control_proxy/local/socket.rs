//! User-owned Unix socket transport. No network listener, detached command queue, or audio work.
use std::future::Future;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use std::collections::HashMap;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::meters::{TelemetryHub, TelemetrySource};
use super::outbound::Outbound;
use super::{LocalControl, Request, RequestKind};

use crate::control_channel::FRAME_LIMIT;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// A connection with nothing in flight and no subscription closes after this long; clients
/// reuse an idle connection for a shorter window.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct LocalSocket {
    stop: watch::Sender<bool>,
    done: watch::Receiver<bool>,
    operation_liveness: Arc<tokio::sync::Notify>,
}

/// Command execution and engine telemetry, injected together at the socket boundary.
trait Dispatch: TelemetrySource {
    fn dispatch(&self, request: Request) -> impl Future<Output = serde_json::Value> + Send;
    fn operation_liveness(&self) -> Arc<tokio::sync::Notify>;
}

impl Dispatch for LocalControl {
    async fn dispatch(&self, request: Request) -> serde_json::Value {
        self.dispatch(request).await
    }

    fn operation_liveness(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.client.operation_liveness)
    }
}

impl Dispatch for Arc<LocalControl> {
    async fn dispatch(&self, request: Request) -> serde_json::Value {
        self.as_ref().dispatch(request).await
    }
    fn operation_liveness(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.client.operation_liveness)
    }
}

impl LocalSocket {
    /// Binds only within an owner-only directory. Existing live sockets are never replaced.
    pub(crate) async fn start(
        directory: &Path,
        owner_uid: u32,
        control: Arc<LocalControl>,
    ) -> io::Result<Self> {
        Self::start_with_dispatch(directory, owner_uid, control).await
    }

    async fn start_with_dispatch(
        directory: &Path,
        owner_uid: u32,
        control: impl Dispatch,
    ) -> io::Result<Self> {
        let (listener, path) =
            mixrelay_local_socket::bind(directory, "control.sock", owner_uid).await?;
        let operation_liveness = control.operation_liveness();
        let (stop, stopped) = watch::channel(false);
        let (finished, done) = watch::channel(false);
        tokio::spawn(async move {
            serve(listener, Arc::new(control), stopped, owner_uid).await;
            drop(path);
            let _ = finished.send(true);
        });
        Ok(Self {
            stop,
            done,
            operation_liveness,
        })
    }

    /// Stop admission, cancel idle readers and join accepted commands before releasing engine ownership.
    pub(crate) async fn shutdown(&self) -> Result<(), String> {
        let _ = self.stop.send(true);
        let mut done = self.done.clone();
        super::super::wait_for_controls(
            async {
                while !*done.borrow() {
                    done.changed().await.map_err(|error| error.to_string())?;
                }
                Ok::<(), String>(())
            },
            &self.operation_liveness,
        )
        .await?
    }
}

impl Drop for LocalSocket {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

async fn serve<C: Dispatch>(
    listener: UnixListener,
    control: Arc<C>,
    mut stop: watch::Receiver<bool>,
    uid: u32,
) {
    let mut clients = JoinSet::new();
    let meters = TelemetryHub::new(Arc::clone(&control), stop.clone());
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            Some(_) = clients.join_next(), if !clients.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { break; };
                if clients.len() >= 32 || !stream.peer_cred().is_ok_and(|peer| peer.uid() == uid) { continue; }
                let control = Arc::clone(&control);
                let meters = Arc::clone(&meters);
                let stopped = stop.clone();
                clients.spawn(async move { let _ = connection(stream, control, meters, stopped).await; });
            }
        }
    }
    drop(listener);
    while clients.join_next().await.is_some() {}
}

/// Requests one editor connection may have in flight or awaiting their reply's write; it
/// stops reading until one finishes.
const IN_FLIGHT_LIMIT: usize = 16;

/// Serves one editor link: requests run concurrently, replies and telemetry share one
/// writer. Reads end when the editor leaves or the runtime stops; mutations always run
/// to completion, and the writer flushes what is queued before the connection closes.
async fn connection<C: Dispatch>(
    stream: UnixStream,
    control: Arc<C>,
    meters: Arc<TelemetryHub<C>>,
    stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let outbound = Arc::new(Outbound::default());
    let writer = tokio::spawn({
        let outbound = Arc::clone(&outbound);
        async move { outbound.write_all(&mut write, IO_TIMEOUT).await }
    });
    let mut requests = Requests {
        control,
        meters,
        outbound: Arc::clone(&outbound),
        reads: JoinSet::new(),
        mutations: JoinSet::new(),
        streams: HashMap::new(),
    };
    let result = requests.serve(read, stop).await;
    requests.reads.abort_all();
    for (id, stream) in requests.streams.drain() {
        stream.abort();
        outbound.cancel(id);
    }
    while requests.mutations.join_next().await.is_some() {}
    outbound.finish();
    let _ = writer.await;
    result
}

struct Requests<C: Dispatch> {
    control: Arc<C>,
    meters: Arc<TelemetryHub<C>>,
    outbound: Arc<Outbound>,
    reads: JoinSet<()>,
    mutations: JoinSet<()>,
    streams: HashMap<u64, tokio::task::AbortHandle>,
}

impl<C: Dispatch> Requests<C> {
    fn in_flight(&self) -> usize {
        self.reads.len() + self.mutations.len()
    }

    async fn serve(
        &mut self,
        read: tokio::net::unix::OwnedReadHalf,
        mut stop: watch::Receiver<bool>,
    ) -> io::Result<()> {
        let mut read = BufReader::new(read.take(FRAME_LIMIT + 1));
        // A cancelled read keeps its partial bytes here, so a frame is never split.
        let mut frame = Vec::new();
        loop {
            if frame.is_empty() {
                read.get_mut().set_limit(FRAME_LIMIT + 1);
            }
            let idle = self.in_flight() == 0 && self.streams.is_empty();
            // Unwritten replies count too, so a slow editor cannot grow the backlog.
            let busy = self.in_flight() + self.outbound.backlog() >= IN_FLIGHT_LIMIT;
            let room = IN_FLIGHT_LIMIT.saturating_sub(self.in_flight());
            let outbound = Arc::clone(&self.outbound);
            let received = tokio::select! {
                biased;
                _ = stop.changed() => return Ok(()),
                () = outbound.closed() => return Ok(()),
                Some(_) = self.mutations.join_next(), if !self.mutations.is_empty() => continue,
                Some(_) = self.reads.join_next(), if !self.reads.is_empty() => continue,
                () = outbound.backlog_below(room), if busy => continue,
                received = read_frame(&mut read, &mut frame, idle), if !busy => received?,
            };
            if received == 0
                || *stop.borrow()
                || frame.len() as u64 > FRAME_LIMIT
                || frame.last() != Some(&b'\n')
            {
                return Ok(());
            }
            let request: Request = serde_json::from_slice(&frame).map_err(io::Error::other)?;
            frame.clear();
            self.start(request, &stop);
        }
    }

    fn start(&mut self, request: Request, stop: &watch::Receiver<bool>) {
        let id = request.id;
        match request.kind() {
            RequestKind::Cancel(subscription) => {
                if let Some(stream) = self.streams.remove(&subscription) {
                    stream.abort();
                }
                self.outbound.cancel(subscription);
            }
            RequestKind::Subscribe => {
                self.outbound.open(id);
                let pump =
                    super::meters::pump(id, Arc::clone(&self.meters), Arc::clone(&self.outbound));
                if let Some(old) = self.streams.insert(id, tokio::spawn(pump).abort_handle()) {
                    old.abort();
                }
            }
            RequestKind::Mutation => {
                let control = Arc::clone(&self.control);
                let outbound = Arc::clone(&self.outbound);
                self.mutations.spawn(async move {
                    dispatch_pending(id, &outbound, control.dispatch(request), HEARTBEAT_INTERVAL)
                        .await;
                });
            }
            RequestKind::Read => {
                let control = Arc::clone(&self.control);
                let outbound = Arc::clone(&self.outbound);
                let mut stop = stop.clone();
                self.reads.spawn(async move {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => {}
                        () = dispatch_pending(id, &outbound, control.dispatch(request), HEARTBEAT_INTERVAL) => {},
                    }
                });
            }
        }
    }
}

/// Reads one frame; an idle connection closes after [`IDLE_TIMEOUT`].
async fn read_frame<R>(read: &mut R, frame: &mut Vec<u8>, idle: bool) -> io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    if !idle {
        return read.read_until(b'\n', frame).await;
    }
    tokio::time::timeout(IDLE_TIMEOUT, read.read_until(b'\n', frame))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

/// Reports liveness while an accepted request waits for its terminal result. The caller
/// cancels reads on shutdown or disconnect; accepted mutations continue to completion.
async fn dispatch_pending<F>(
    request_id: u64,
    outbound: &Outbound,
    dispatch: F,
    heartbeat_interval: Duration,
) where
    F: Future<Output = serde_json::Value>,
{
    let mut dispatch = Box::pin(dispatch);
    let first_heartbeat = tokio::time::Instant::now() + heartbeat_interval;
    let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_interval);
    loop {
        tokio::select! {
            response = &mut dispatch => return outbound.reply(response),
            _ = heartbeat.tick() => {
                outbound.reply(serde_json::json!({"id": request_id, "status": 102, "body": {}}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::os::unix::fs::MetadataExt as _;
    use tokio::io::AsyncWriteExt as _;
    use tokio::sync::oneshot;

    #[derive(Default)]
    struct HeldControl {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
        completed: std::sync::atomic::AtomicBool,
    }

    struct NoTelemetry;

    impl super::super::meters::TelemetryChunks for NoTelemetry {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
            std::future::pending().await
        }
    }

    impl TelemetrySource for Arc<HeldControl> {
        type Chunks = NoTelemetry;

        async fn open(&self) -> Result<NoTelemetry, String> {
            Ok(NoTelemetry)
        }
    }

    impl Dispatch for Arc<HeldControl> {
        async fn dispatch(&self, request: Request) -> Value {
            self.started.notify_one();
            self.release.notified().await;
            self.completed
                .store(true, std::sync::atomic::Ordering::Release);
            serde_json::json!({"id": request.id, "status": 200, "body": {}})
        }

        fn operation_liveness(&self) -> Arc<tokio::sync::Notify> {
            Arc::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_live_slow_read_reports_pending_before_the_editor_liveness_deadline() {
        let fixture = mixrelay_test_support::FixtureDir::ipc("ctl").unwrap();
        let directory = fixture.path().to_owned();
        let uid = std::fs::metadata(fixture.path()).unwrap().uid();
        let control = Arc::new(HeldControl::default());
        let service = LocalSocket::start_with_dispatch(&directory, uid, Arc::clone(&control))
            .await
            .unwrap();
        let mut stream = UnixStream::connect(directory.join("control.sock"))
            .await
            .unwrap();
        stream
            .write_all(b"{\"id\":7,\"command\":{\"type\":\"invoke\",\"name\":\"engine_request\",\"arguments\":{\"method\":\"GET\",\"path\":\"/api/v1/graph\",\"body\":null}}}\n")
            .await
            .unwrap();
        control.started.notified().await;
        let mut reader = BufReader::new(stream);
        for _ in 0..4 {
            let mut frame = String::new();
            tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut frame))
                .await
                .expect("a live accepted read must answer before the editor declares it silent")
                .unwrap();
            let frame: Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(
                (frame["id"].as_u64(), frame["status"].as_u64()),
                (Some(7), Some(102))
            );
            assert!(!control.completed.load(std::sync::atomic::Ordering::Acquire));
        }
        control.release.notify_one();
        let mut frame = String::new();
        reader.read_line(&mut frame).await.unwrap();
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(
            (frame["id"].as_u64(), frame["status"].as_u64()),
            (Some(7), Some(200))
        );
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn quit_cancels_async_reads_but_joins_accepted_writes_and_native_discovery() {
        for method in ["GET", "PUT", "applications"] {
            let must_complete =
                method == "PUT" || (method == "applications" && cfg!(target_os = "linux"));
            let fixture = mixrelay_test_support::FixtureDir::ipc("ctl").unwrap();
            let directory = fixture.path().to_owned();
            let uid = std::fs::metadata(fixture.path()).unwrap().uid();
            let control = Arc::new(HeldControl::default());
            let service = LocalSocket::start_with_dispatch(&directory, uid, Arc::clone(&control))
                .await
                .unwrap();
            let mut stream = UnixStream::connect(directory.join("control.sock"))
                .await
                .unwrap();
            let mut request = serde_json::json!({"id":1,"command":{
                "type":"invoke","name":"engine_request",
                "arguments":{"method":method,"path":"/api/v1/graph","body":null}
            }});
            if method == "applications" {
                request["command"] = serde_json::json!({
                    "type":"invoke", "name":"list_capturable_applications", "arguments":null
                });
            }
            stream
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
            control.started.notified().await;
            let shutdown = tokio::spawn(async move { service.shutdown().await });
            if must_complete {
                // An accepted write or owned native query must finish before shutdown.
                tokio::task::yield_now().await;
                assert!(!shutdown.is_finished());
                assert!(!control.completed.load(std::sync::atomic::Ordering::Acquire));
                control.release.notify_one();
            }
            tokio::time::timeout(Duration::from_secs(1), shutdown)
                .await
                .expect("shutdown must finish after asynchronous reads are cancelled and owned work completes")
                .unwrap()
                .unwrap();
            assert_eq!(
                control.completed.load(std::sync::atomic::Ordering::Acquire),
                must_complete
            );
            assert!(!directory.join("control.sock").exists());
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_idle_readers_and_removes_only_owned_socket() {
        let fixture = mixrelay_test_support::FixtureDir::ipc("ctl").expect("fixture");
        let directory = fixture.path().to_owned();
        let uid = std::fs::metadata(fixture.path()).expect("metadata").uid();
        let control = LocalControl::new(
            crate::control_proxy::ControlProxyClient::with_socket(Some(
                fixture.join("no-engine.sock"),
            )),
            #[cfg(target_os = "macos")]
            Arc::default(),
        )
        .expect("control");
        let service = LocalSocket::start(&directory, uid, Arc::new(control))
            .await
            .expect("service");
        let mut client = UnixStream::connect(directory.join("control.sock"))
            .await
            .expect("connect");
        client
            .write_all(b"{\"id\":")
            .await
            .expect("incomplete frame");
        tokio::time::timeout(Duration::from_secs(1), service.shutdown())
            .await
            .expect("bounded shutdown")
            .expect("shutdown");
        assert!(!directory.join("control.sock").exists());
        service.shutdown().await.expect("idempotent shutdown");
    }

    #[tokio::test]
    async fn mutation_emits_pending_then_terminal_frame() {
        let outbound = Outbound::default();
        let dispatch = async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            serde_json::json!({"id": 7, "status": 200, "body": {"accepted": true}})
        };
        dispatch_pending(7, &outbound, dispatch, Duration::from_millis(5)).await;
        outbound.finish();
        let (mut runtime, mut peer) = tokio::io::duplex(4096);
        outbound
            .write_all(&mut runtime, Duration::from_secs(1))
            .await
            .expect("frames");
        drop(runtime);

        let mut output = Vec::new();
        peer.read_to_end(&mut output).await.expect("read frames");
        let frames = output
            .split(|byte| *byte == b'\n')
            .filter(|frame| !frame.is_empty())
            .map(|frame| serde_json::from_slice::<Value>(frame).expect("json frame"))
            .collect::<Vec<_>>();
        assert!(frames.iter().any(|frame| frame["status"] == 102));
        assert_eq!(
            frames.last(),
            Some(&serde_json::json!({
                "id": 7,
                "status": 200,
                "body": {"accepted": true}
            }))
        );
    }

    #[tokio::test]
    async fn a_closed_link_does_not_cancel_an_accepted_mutation() {
        let outbound = Outbound::default();
        outbound.close();
        let (completed, received) = oneshot::channel();
        let dispatch = async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = completed.send(());
            serde_json::json!({"id": 8, "status": 200, "body": {}})
        };
        dispatch_pending(8, &outbound, dispatch, Duration::from_millis(5)).await;
        tokio::time::timeout(Duration::from_millis(100), received)
            .await
            .expect("mutation completion")
            .expect("completion signal");
    }

    #[tokio::test]
    async fn one_connection_runs_commands_reads_and_a_subscription_together() {
        let fixture = mixrelay_test_support::FixtureDir::ipc("ctl").expect("fixture");
        let directory = fixture.path().to_owned();
        let uid = std::fs::metadata(fixture.path()).expect("metadata").uid();
        let control = LocalControl::new(
            crate::control_proxy::ControlProxyClient::with_socket(Some(
                fixture.join("no-engine.sock"),
            )),
            #[cfg(target_os = "macos")]
            Arc::default(),
        )
        .expect("control");
        let service = LocalSocket::start(&directory, uid, Arc::new(control))
            .await
            .expect("service");
        let stream = UnixStream::connect(directory.join("control.sock"))
            .await
            .expect("connect");
        let mut reader = BufReader::new(stream);
        let requests = [
            serde_json::json!({"id": 9, "command": {"type": "cancel", "subscription": 99}}),
            serde_json::json!({"id": 1, "command": {"type": "invoke", "name": "hello", "arguments": {}}}),
            serde_json::json!({"id": 2, "command": {"type": "subscribe_telemetry"}}),
            serde_json::json!({"id": 3, "command": {"type": "snapshot"}}),
        ];
        let batch = requests
            .iter()
            .map(|request| format!("{request}\n"))
            .collect::<Vec<_>>()
            .concat();
        reader
            .get_mut()
            .write_all(batch.as_bytes())
            .await
            .expect("requests");
        let mut answered = std::collections::BTreeMap::new();
        while answered.len() < 3 {
            let mut reply = String::new();
            reader.read_line(&mut reply).await.expect("reply");
            let reply: Value = serde_json::from_str(&reply).expect("json");
            answered.insert(reply["id"].as_u64().expect("id"), reply["status"].clone());
        }
        // No engine: every request fails on its own; the cancel has no reply.
        assert_eq!(answered.keys().copied().collect::<Vec<_>>(), [1, 2, 3]);
        assert!(answered.values().all(|status| status == 503));
        drop(reader);
        tokio::time::timeout(Duration::from_secs(1), service.shutdown())
            .await
            .expect("bounded shutdown")
            .expect("shutdown");
    }

    #[tokio::test]
    async fn one_connection_carries_sequential_requests_until_the_peer_closes() {
        let fixture = mixrelay_test_support::FixtureDir::ipc("ctl").expect("fixture");
        let directory = fixture.path().to_owned();
        let uid = std::fs::metadata(fixture.path()).expect("metadata").uid();
        let control = LocalControl::new(
            crate::control_proxy::ControlProxyClient::with_socket(Some(
                fixture.join("no-engine.sock"),
            )),
            #[cfg(target_os = "macos")]
            Arc::default(),
        )
        .expect("control");
        let service = LocalSocket::start(&directory, uid, Arc::new(control))
            .await
            .expect("service");
        let stream = UnixStream::connect(directory.join("control.sock"))
            .await
            .expect("connect");
        let mut reader = BufReader::new(stream);
        for id in [1, 2] {
            let request = serde_json::json!({"id": id, "command": {"type": "invoke", "name": "hello", "arguments": {}}});
            reader
                .get_mut()
                .write_all(format!("{request}\n").as_bytes())
                .await
                .expect("request");
            let mut reply = String::new();
            reader.read_line(&mut reply).await.expect("reply");
            let reply: Value = serde_json::from_str(&reply).expect("json");
            assert_eq!(reply["id"], id);
            assert_eq!(reply["status"], 503);
        }
        drop(reader);
        tokio::time::timeout(Duration::from_secs(1), service.shutdown())
            .await
            .expect("bounded shutdown")
            .expect("shutdown");
    }
}
