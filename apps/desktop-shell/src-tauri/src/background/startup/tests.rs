use super::*;
use crate::background::fixture_pipe::Pipe;
use mixrelay_desktop_core::timing::AsyncDelay;
use mixrelay_time::{Deadline, MonotonicClock};
use serde_json::json;
use std::os::unix::fs::MetadataExt as _;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    net::UnixListener,
};

/// Runs the connection policy 100 times faster than wall-clock time, so launch retries and
/// probe intervals take milliseconds. The fixtures are real processes and sockets.
struct FastTime {
    origin: std::time::Instant,
}

const SPEEDUP: u32 = 100;

impl FastTime {
    fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }

    async fn wait(&self, duration: Duration) {
        self.wait_until(Deadline::after(self.now(), duration)).await;
    }
}

impl MonotonicClock for FastTime {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(self.origin.elapsed() * SPEEDUP)
    }
}

impl AsyncDelay for FastTime {
    async fn wait_until(&self, deadline: Deadline) {
        tokio::time::sleep(deadline.remaining(self.now()) / SPEEDUP).await;
    }
}

/// Connects within `timeout` of fast time. Each exchange with an in-process fixture server
/// keeps seconds of real headroom, so only the connection budget can expire.
fn options(timeout: Duration) -> runtime::ConnectOptions<'static> {
    runtime::ConnectOptions {
        compatibility: compatibility(),
        timeout,
        exchange_timeout: Duration::from_mins(5),
    }
}

fn hello() -> Value {
    json!({"protocolVersion":crate::control_channel::LINK_PROTOCOL,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")})
}

/// Answers every request until the editor closes its link; the runtime keeps links open.
async fn serve_connection(listener: UnixListener, status: u16) {
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut frame = String::new();
        while reader.read_line(&mut frame).await.unwrap() != 0 {
            let request: Value = serde_json::from_str(&frame).unwrap();
            frame.clear();
            let body = if request["command"]["name"] == "hello" {
                hello()
            } else {
                json!({"status":status})
            };
            reader
                .get_mut()
                .write_all(
                    format!(
                        "{}\n",
                        json!({"id":request["id"],"status":200,"body":{"ok":body}})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    }
}

fn fixture(directory: &std::path::Path, script: &str) -> (Connection, PathBuf) {
    let executable = directory.join("controller.sh");
    mixrelay_test_support::write_executable(
        &executable,
        &format!("#!/bin/sh\ncd \"${{0%/*}}\"\n{script}\n"),
    )
    .unwrap();
    (socket_connection(directory), executable)
}

fn socket_connection(directory: &std::path::Path) -> Connection {
    Connection::at_socket(
        directory.join("control.sock"),
        std::fs::metadata(directory).unwrap().uid(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    )
}

#[tokio::test]
async fn exited_first_launch_is_retried_during_the_same_explicit_connection() {
    let directory = tempfile::tempdir().unwrap();
    // The relaunched controller stays alive until its runtime socket is bound, so it is
    // never observed as exited and relaunched before it can answer.
    let (connection, executable) = fixture(
        directory.path(),
        "printf attempt >> attempts\nif test ! -f first; then touch first; exit 2; fi\ntouch ready\nread -r _ < bound",
    );
    let path = directory.path().join("control.sock");
    let ready = directory.path().join("ready");
    let mut bound = Pipe::create(&directory.path().join("bound"));
    let time = std::sync::Arc::new(FastTime::new());
    let server_time = std::sync::Arc::clone(&time);
    let server = tokio::spawn(async move {
        while !ready.exists() {
            server_time.wait(Duration::from_millis(10)).await;
        }
        let listener = UnixListener::bind(path).unwrap();
        bound.send("bound\n");
        serve_connection(listener, 200).await;
    });
    let stages = std::sync::Mutex::new(Vec::new());
    assert_eq!(
        runtime::connect_until_ready(
            &connection,
            &Launcher {
                executable: &executable,
                admission: || Ok(true),
            },
            time.as_ref(),
            options(Duration::from_mins(1)),
            |stage| stages.lock().unwrap().push(stage)
        )
        .await
        .unwrap(),
        RuntimeStatus::Ready
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("attempts")).unwrap(),
        "attemptattempt"
    );
    // A retried launch is still one Starting step, then its answering runtime starts the engine.
    assert_eq!(
        *stages.lock().unwrap(),
        [ConnectStage::Starting, ConnectStage::PreparingEngine]
    );
    server.abort();
}

#[tokio::test]
async fn busy_owner_and_a_living_failed_engine_are_never_duplicated() {
    let directory = tempfile::tempdir().unwrap();
    let (connection, executable) = fixture(directory.path(), "touch launched");
    let time = FastTime::new();
    assert_eq!(
        runtime::connect_until_ready(
            &connection,
            &Launcher {
                executable: &executable,
                admission: || Ok(false),
            },
            &time,
            options(Duration::from_secs(1)),
            |_| {}
        )
        .await
        .unwrap(),
        RuntimeStatus::Unavailable
    );
    assert!(!directory.path().join("launched").exists());
    let server = tokio::spawn(serve_connection(
        UnixListener::bind(directory.path().join("control.sock")).unwrap(),
        503,
    ));
    let stages = std::sync::Mutex::new(Vec::new());
    assert_eq!(
        runtime::connect_until_ready(
            &connection,
            &Launcher {
                executable: &executable,
                admission: || Ok(true),
            },
            &time,
            options(Duration::from_secs(5)),
            |stage| stages.lock().unwrap().push(stage)
        )
        .await
        .unwrap(),
        RuntimeStatus::Unavailable
    );
    assert!(!directory.path().join("launched").exists());
    // The answering runtime owns its engine start; it is reported once, never relaunched.
    assert_eq!(*stages.lock().unwrap(), [ConnectStage::PreparingEngine]);
    server.abort();
}

#[tokio::test]
async fn a_slow_but_healthy_engine_is_reported_as_being_prepared() {
    let directory = tempfile::tempdir().unwrap();
    let (connection, executable) = fixture(directory.path(), "touch launched");
    let listener = UnixListener::bind(directory.path().join("control.sock")).unwrap();
    let time = std::sync::Arc::new(FastTime::new());
    let server_time = std::sync::Arc::clone(&time);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        for (result, delay) in [(hello(), 0), (json!({"status":200}), 300)] {
            let mut frame = String::new();
            reader.read_line(&mut frame).await.unwrap();
            let request: Value = serde_json::from_str(&frame).unwrap();
            server_time.wait(Duration::from_millis(delay)).await;
            reader
                .get_mut()
                .write_all(
                    format!(
                        "{}\n",
                        json!({"id":request["id"],"status":200,"body":{"ok":result}})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let stages = std::sync::Mutex::new(Vec::new());
    assert_eq!(
        runtime::connect_until_ready(
            &connection,
            &Launcher {
                executable: &executable,
                admission: || Ok(true),
            },
            time.as_ref(),
            options(Duration::from_mins(1)),
            |stage| stages.lock().unwrap().push(stage)
        )
        .await
        .unwrap(),
        RuntimeStatus::Ready
    );
    assert_eq!(*stages.lock().unwrap(), [ConnectStage::PreparingEngine]);
    assert!(!directory.path().join("launched").exists());
    server.await.unwrap();
}

#[tokio::test]
async fn a_broken_executable_has_a_fixed_retry_limit() {
    let directory = tempfile::tempdir().unwrap();
    let (connection, executable) = fixture(directory.path(), "printf attempt >> attempts\nexit 2");
    assert_eq!(
        runtime::connect_until_ready(
            &connection,
            &Launcher {
                executable: &executable,
                admission: || Ok(true),
            },
            &FastTime::new(),
            options(Duration::from_mins(1)),
            |_| {}
        )
        .await
        .unwrap(),
        RuntimeStatus::Unavailable
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("attempts")).unwrap(),
        "attemptattemptattempt"
    );
}

#[test]
fn old_app_controller_and_future_protocol_are_not_driver_connections() {
    for hello in [
        json!({"protocolVersion":1,"version":"0.1.2"}),
        json!({"protocolVersion":2,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")}),
        json!({"protocolVersion":2,"product":"mixrelay-app","version":"0.1.2"}),
        json!({"protocolVersion":4,"product":"mixrelay-driver","version":env!("CARGO_PKG_VERSION")}),
        json!({"protocolVersion":2,"product":"mixrelay-driver","version":"0.1.1"}),
        json!({"protocolVersion":2,"product":"mixrelay-driver","version":"0.1.2"}),
        json!({"protocolVersion":2,"product":"mixrelay-driver","version":"0.2.0"}),
    ] {
        assert!(!compatibility().accepts(&hello));
    }
    assert!(compatibility().accepts(&hello()));
}

#[tokio::test]
async fn controller_with_unavailable_engine_never_opens_editor() {
    for code in [200, 503] {
        let directory = tempfile::tempdir().unwrap();
        let connection = socket_connection(directory.path());
        let listener = UnixListener::bind(directory.path().join("control.sock")).unwrap();
        let server = tokio::spawn(async move {
            // The controller keeps a connection open for the next request.
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            for result in [hello(), json!({"status":code})] {
                let mut frame = String::new();
                reader.read_line(&mut frame).await.unwrap();
                let request: Value = serde_json::from_str(&frame).unwrap();
                reader
                    .get_mut()
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"id":request["id"],"status":200,"body":{"ok":result}})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        assert_eq!(
            readiness(&connection).await,
            if code == 200 {
                RuntimeStatus::Ready
            } else {
                RuntimeStatus::Unavailable
            }
        );
        server.await.unwrap();
    }
}
