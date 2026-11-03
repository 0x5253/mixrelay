#[cfg(feature = "native-lab")]
use serde_json::Value;
use std::path::Path;
#[cfg(feature = "native-lab")]
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(feature = "native-lab")]
const PHASE_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(feature = "native-lab")]
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(feature = "native-lab")]
const CONTROL_LINE_LIMIT: usize = 1024;

#[cfg(feature = "native-lab")]
pub(super) struct FixtureChild {
    child: Child,
    child_id: u32,
    input: ChildStdin,
    events: Option<Receiver<Result<Value, String>>>,
    output_reader: Option<JoinHandle<()>>,
    exit_status: Option<ExitStatus>,
}

#[cfg(feature = "native-lab")]
impl FixtureChild {
    pub(super) fn spawn(path: &Path, arguments: &[&str], environment: &[(&str, String)]) -> Self {
        let mut command = Command::new(path);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (name, value) in environment {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("owned native mute fixture");
        let child_id = child.id();
        let input = child.stdin.take().expect("fixture stdin");
        let output = child.stdout.take().expect("fixture stdout");
        let (sender, events) = sync_channel(8);
        let output_reader = Some(thread::spawn(move || read_events(output, &sender)));
        Self {
            child,
            child_id,
            input,
            events: Some(events),
            output_reader,
            exit_status: None,
        }
    }

    pub(super) fn send(&mut self, command: &str) {
        writeln!(self.input, "{command}").expect("write fixture command");
        self.input.flush().expect("flush fixture command");
    }

    pub(super) fn event(&self, name: &str, phase: Option<usize>) -> Value {
        let deadline = Instant::now() + PHASE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = match self
                .events
                .as_ref()
                .expect("fixture event receiver is live")
                .recv_timeout(remaining)
            {
                Ok(Ok(event)) => event,
                Ok(Err(error)) => panic!("invalid fixture control event: {error}"),
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for {name}"),
                Err(RecvTimeoutError::Disconnected) => panic!("fixture closed before {name}"),
            };
            if event["event"] == name && phase.is_none_or(|expected| event["phase"] == expected) {
                return event;
            }
            assert_ne!(
                event["event"], "error",
                "fixture reported an error: {event}"
            );
        }
    }

    pub(super) fn stop(&mut self) -> ExitStatus {
        self.request_quit();
        self.wait()
    }

    pub(super) fn id(&self) -> u32 {
        self.child_id
    }

    fn request_quit(&mut self) {
        if self.exit_status.is_none() {
            let _ = writeln!(self.input, "quit");
            let _ = self.input.flush();
        }
    }

    pub(super) fn wait(&mut self) -> ExitStatus {
        if self.exit_status.is_none() {
            self.exit_status = Some(
                wait_for_child(&mut self.child, self.child_id).expect("reap owned fixture child"),
            );
            self.events.take();
            if let Some(reader) = self.output_reader.take() {
                reader.join().expect("fixture stdout reader");
            }
        }
        self.exit_status.unwrap()
    }
}

#[cfg(feature = "native-lab")]
impl Drop for FixtureChild {
    fn drop(&mut self) {
        if self.exit_status.is_none() {
            self.events.take();
            let _ = writeln!(self.input, "quit");
            let _ = self.input.flush();
            if wait_for_child(&mut self.child, self.child_id).is_ok()
                && let Some(reader) = self.output_reader.take()
            {
                let _ = reader.join();
            }
        }
    }
}

#[cfg(feature = "native-lab")]
fn wait_for_child(child: &mut Child, child_id: u32) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + CHILD_EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            if !is_owned_child(child_id, child.id()) {
                return Err(io::Error::other(
                    "refusing to terminate an unowned fixture process",
                ));
            }
            child.kill()?;
            return child.wait();
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn is_owned_child(expected: u32, actual: u32) -> bool {
    expected != 0 && expected == actual
}

#[cfg(feature = "native-lab")]
fn read_events(mut output: ChildStdout, sender: &SyncSender<Result<Value, String>>) {
    loop {
        let mut line = Vec::with_capacity(128);
        loop {
            let mut byte = [0_u8; 1];
            match output.read(&mut byte) {
                Ok(0) => return,
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) if line.len() < CONTROL_LINE_LIMIT => line.push(byte[0]),
                Ok(_) => {
                    let _ = sender.send(Err("fixture control line exceeded limit".into()));
                    return;
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            }
        }
        let event = serde_json::from_slice(&line).map_err(|error| error.to_string());
        if sender.send(event).is_err() {
            return;
        }
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn fixture_paths() -> (PathBuf, PathBuf, PathBuf) {
    let source = PathBuf::from(
        std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE")
            .expect("explicit test-owned application source"),
    );
    let sink = PathBuf::from(
        std::env::var("MIXRELAY_LAB_APPLICATION_MUTE_SINK")
            .expect("explicit test-owned mute sink observer"),
    );
    let directory = source.parent().expect("fixture parent").to_owned();
    assert!(source.is_absolute() && sink.is_absolute());
    assert_eq!(sink.parent(), Some(directory.as_path()));
    assert!(
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("app-lab-")),
        "fixture must live in the runner's private app-lab directory"
    );
    let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").expect("private lab runtime"));
    assert!(
        is_private_fixture_scope(&source, &sink, &runtime),
        "fixtures must share the runner's private runtime"
    );
    (source, sink, directory)
}

fn is_private_fixture_scope(source: &Path, sink: &Path, runtime: &Path) -> bool {
    if !source.is_absolute() || !sink.is_absolute() || source.parent() != sink.parent() {
        return false;
    }
    let Some(directory) = source.parent() else {
        return false;
    };
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("app-lab-"))
        && source.file_name().and_then(|name| name.to_str()) == Some("application-fixture")
        && sink.file_name().and_then(|name| name.to_str()) == Some("mute-sink-fixture")
        && runtime == directory.join("runtime")
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    #[test]
    fn cleanup_target_must_remain_the_exact_nonzero_child_pid() {
        assert!(is_owned_child(42, 42));
        assert!(!is_owned_child(0, 0));
        assert!(!is_owned_child(42, 43));
    }

    #[test]
    fn fixture_paths_must_share_the_private_runtime_and_lab_directory() {
        let source = Path::new("/tmp/app-lab-owned/application-fixture");
        let sink = Path::new("/tmp/app-lab-owned/mute-sink-fixture");
        let runtime = Path::new("/tmp/app-lab-owned/runtime");
        assert!(is_private_fixture_scope(source, sink, runtime));
        assert!(!is_private_fixture_scope(
            Path::new("/tmp/user-session/application-fixture"),
            sink,
            runtime
        ));
        assert!(!is_private_fixture_scope(
            source,
            Path::new("/tmp/app-lab-other/mute-sink-fixture"),
            runtime
        ));
        assert!(!is_private_fixture_scope(
            source,
            sink,
            Path::new("/tmp/app-lab-owned/other-runtime")
        ));
    }
}
