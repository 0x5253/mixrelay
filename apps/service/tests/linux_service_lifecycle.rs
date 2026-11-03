//! Production executable checks confined to the owned private `PipeWire` harness.
#![cfg(all(target_os = "linux", feature = "native-lab"))]

use mixrelay_audio_io::process::ProcessingState;
use mixrelay_control_protocol::AppStatus;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

/// The controller-owned engine binds the session's socket directory (the controller drops
/// any `MIXRELAY_IPC_DIR` override), which the harness confines to the lab's runtime directory.
fn engine_socket() -> PathBuf {
    mixrelay_local_socket::session_socket_directory()
        .unwrap()
        .join(mixrelay_local_socket::ENGINE_SOCKET)
}

fn assert_private_socket_unused(lab: &Path) {
    assert!(
        engine_socket().starts_with(lab),
        "engine socket {} is outside the lab",
        engine_socket().display()
    );
    assert_socket_removed();
}

fn assert_socket_removed() {
    let socket = engine_socket();
    assert!(
        !socket.try_exists().unwrap(),
        "engine left {}",
        socket.display()
    );
    assert!(
        UnixStream::connect(&socket).is_err(),
        "control socket is still connectable"
    );
}

#[test]
fn native_linux_service_desktop_owned_shutdown() {
    let fixture = PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let lab = fixture.parent().unwrap();
    assert!(
        lab.to_string_lossy()
            .contains("/.lab/mixrelay-integration-")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()),
        lab.join("config")
    );
    assert_private_socket_unused(lab);
    let executable = Path::new(env!("CARGO_BIN_EXE_mixrelay-service"));
    let mut engine = mixrelay_engine_process::ControllerEngine::start(
        executable,
        &mixrelay_time::StdClock::new(),
    )
    .unwrap();
    let pid = engine.id();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !processing_is_running() {
        assert!(engine.try_wait().unwrap().is_none());
        assert!(
            Instant::now() < deadline,
            "owned engine never processed a native cycle"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        owned_registry_objects(pid) > 0,
        "native owner was not published"
    );
    // A second launch must fail its own handshake, not adopt or stop this listener.
    assert!(
        mixrelay_engine_process::ControllerEngine::start(
            executable,
            &mixrelay_time::StdClock::new()
        )
        .is_err()
    );
    assert!(engine.try_wait().unwrap().is_none());
    assert!(processing_is_running());
    engine.request_shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = engine.try_wait().unwrap() {
            assert!(
                status.success(),
                "owned engine finalization failed: {status}"
            );
            break;
        }
        assert!(Instant::now() < deadline, "owner EOF did not stop engine");
        thread::sleep(Duration::from_millis(10));
    }
    assert_socket_removed();
    let deadline = Instant::now() + Duration::from_secs(3);
    while owned_registry_objects(pid) != 0 {
        assert!(
            Instant::now() < deadline,
            "owned engine left native objects"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

struct Engine {
    child: Child,
    log: PathBuf,
}

impl Engine {
    fn start(lab: &Path, label: &str) -> Self {
        let log = lab.join(format!("{label}.log"));
        let child = Command::new(env!("CARGO_BIN_EXE_mixrelay-service"))
            .env("MIXRELAY_CONFIG_DIR", lab.join("engine-config"))
            // Both engines in this file bind the lab session socket (see engine_socket).
            .env_remove("MIXRELAY_IPC_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create_new(&log).unwrap())
            .spawn()
            .unwrap();
        Self { child, log }
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "engine did not exit; log={}",
                self.log.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminate(&mut self) {
        assert!(self.child.try_wait().unwrap().is_none());
        // The still-owned, unreaped child cannot have its PID reused. Signal only
        // this engine, not its process group, the daemon, or another installed app.
        assert!(
            Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        assert!(self.wait().success(), "graceful engine exit failed");
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn get_json(path: &str) -> Option<Value> {
    let timeout = Duration::from_millis(200);
    let mut stream = UnixStream::connect(engine_socket()).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: mixrelay-engine\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.take(65_537).read_to_string(&mut response).ok()?;
    if !response.starts_with("HTTP/1.1 200 ") || response.len() > 65_536 {
        return None;
    }
    serde_json::from_str(response.split_once("\r\n\r\n")?.1).ok()
}

fn wait_running(engine: &mut Engine) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            engine.child.try_wait().unwrap().is_none(),
            "engine exited: {}",
            fs::read_to_string(&engine.log).unwrap()
        );
        if processing_is_running() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "engine never processed a native cycle"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn processing_is_running() -> bool {
    get_json("/api/v1/status")
        .and_then(|value| serde_json::from_value::<AppStatus>(value).ok())
        .and_then(|status| status.audio_session)
        .is_some_and(|session| session.processing_state == ProcessingState::Running)
}

fn owned_registry_objects(pid: u32) -> usize {
    let output = Command::new("pw-dump").output().unwrap();
    assert!(output.status.success());
    let inventory: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    inventory
        .iter()
        .filter(|object| {
            let value = &object["info"]["props"]["application.process.id"];
            value.as_u64().or_else(|| value.as_str()?.parse().ok()) == Some(u64::from(pid))
        })
        .count()
}

#[test]
fn native_linux_service_startup_shutdown() {
    let fixture = PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let lab = fixture.parent().unwrap();
    assert!(fixture.is_file() && fixture.is_absolute());
    assert!(
        lab.to_string_lossy()
            .contains("/.lab/mixrelay-integration-")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()),
        lab.join("runtime")
    );
    assert_private_socket_unused(lab);
    let mut engine = Engine::start(lab, "engine");
    wait_running(&mut engine);
    let graph: mixrelay_audio_graph::GraphDocument =
        serde_json::from_value(get_json("/api/v1/graph").unwrap()).unwrap();
    assert!(graph.nodes.is_empty() && graph.edges.is_empty());
    assert!(
        owned_registry_objects(engine.child.id()) > 0,
        "native graph driver was not published"
    );
    assert!(
        fs::metadata(engine_socket())
            .unwrap()
            .file_type()
            .is_socket()
    );
    let mut rejected = Engine::start(lab, "occupied-socket");
    assert!(!rejected.wait().success());
    assert!(
        fs::read_to_string(&rejected.log)
            .unwrap()
            .contains("local control")
    );
    assert_eq!(owned_registry_objects(rejected.child.id()), 0);
    wait_running(&mut engine);
    let pid = engine.child.id();
    engine.terminate();
    let deadline = Instant::now() + Duration::from_secs(3);
    while owned_registry_objects(pid) != 0 {
        assert!(
            Instant::now() < deadline,
            "native owners survived engine shutdown"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_socket_removed();
    let mut restarted = Engine::start(lab, "restarted-engine");
    wait_running(&mut restarted);
    let restored: mixrelay_audio_graph::GraphDocument =
        serde_json::from_value(get_json("/api/v1/graph").unwrap()).unwrap();
    assert!(
        restored.revision > graph.revision,
        "catalog restore must use a fresh runtime revision"
    );
    assert_eq!(
        restored,
        mixrelay_audio_graph::GraphDocument {
            revision: restored.revision,
            ..graph
        }
    );
    restarted.terminate();
    assert_socket_removed();
    println!(
        "ordinary_startup=running; implicit_nodes=0; occupied_socket=native_owners_0; graceful_exit=0; native_owners_after_exit=0; restart=persisted_empty_graph"
    );
}
