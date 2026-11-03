//! Bounded discovery child capture without pipe-reader threads. A vendor can spawn a child
//! inheriting stdout, so joining a pipe reader after killing the scanner is not a deadline.
//! Private files avoid waiting for inherited pipe EOF; direct children are killed and reaped on
//! every unsuccessful exit. This is not an OS process-tree sandbox: vendor descendants are not
//! owned here.

use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const STDOUT_LIMIT: u64 = 4 * 1024 * 1024;
const STDERR_LIMIT: u64 = 16 * 1024;

pub(super) fn capture_json_child<T: serde::de::DeserializeOwned>(
    command: Command,
    timeout: Duration,
) -> Result<T, String> {
    let directory = crate::worker_paths::worker_directory()
        .map_err(|error| format!("capture directory failed: {error}"))?;
    crate::worker_paths::private_directory(&directory)
        .map_err(|error| format!("capture directory failed: {error}"))?;
    capture_json_child_in(command, timeout, &directory)
}

fn capture_json_child_in<T: serde::de::DeserializeOwned>(
    mut command: Command,
    timeout: Duration,
    directory: &Path,
) -> Result<T, String> {
    let stdout = CaptureFile::new(directory)?;
    let stderr = CaptureFile::new(directory)?;
    let stdout_handle = stdout.stdio()?;
    let stderr_handle = stderr.stdio()?;
    command
        .stdin(Stdio::piped())
        .stdout(stdout_handle)
        .stderr(stderr_handle);
    let spawned = command.spawn();
    // Command retains its configured file handles after spawning; release them before cleanup.
    drop(command);
    let child = spawned.map_err(|error| format!("child spawn failed: {error}"))?;
    let mut child = OwnedChild {
        child,
        reaped: false,
    };
    let status = wait(&mut child, timeout, &stdout, &stderr)?;
    if !status.success() {
        let detail = stderr.read(STDERR_LIMIT).unwrap_or_default();
        return Err(format!(
            "child exited with {status}: {}",
            String::from_utf8_lossy(&detail).trim()
        ));
    }
    let output = stdout.read(STDOUT_LIMIT)?;
    // Protocol output is authoritative data, unlike best-effort human-readable stderr. Never
    // repair malformed UTF-8 into a different plug-in identity or descriptor before parsing.
    serde_json::from_slice(&output).map_err(|error| format!("invalid child JSON: {error}"))
}

fn wait(
    child: &mut OwnedChild,
    timeout: Duration,
    stdout: &CaptureFile,
    stderr: &CaptureFile,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .child
            .try_wait()
            .map_err(|error| format!("child wait failed: {error}"))?
        {
            child.reaped = true;
            // Preserve an actual crash/exit diagnostic even if its output was too large.
            if status.success() {
                stdout.check_size(STDOUT_LIMIT)?;
                stderr.check_size(STDERR_LIMIT)?;
            }
            return Ok(status);
        }
        stdout.check_size(STDOUT_LIMIT)?;
        stderr.check_size(STDERR_LIMIT)?;
        if Instant::now() >= deadline {
            return Err(format!("child timed out after {} ms", timeout.as_millis()));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct OwnedChild {
    child: Child,
    reaped: bool,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The file handle closes before its owned private capture directory is removed.
struct CaptureFile {
    file: File,
    paths: crate::worker_paths::WorkerPaths,
}

impl CaptureFile {
    fn new(directory: &Path) -> Result<Self, String> {
        let paths = crate::worker_paths::WorkerPaths::new_in(directory)
            .map_err(|error| format!("capture directory failed: {error}"))?;
        let path = paths.shared();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|error| format!("capture file failed: {error}"))?;
        Ok(Self { file, paths })
    }

    fn stdio(&self) -> Result<Stdio, String> {
        self.file
            .try_clone()
            .map(Stdio::from)
            .map_err(|error| format!("capture handle failed: {error}"))
    }

    fn check_size(&self, limit: u64) -> Result<(), String> {
        let size = self
            .file
            .metadata()
            .map_err(|error| format!("capture metadata failed: {error}"))?
            .len();
        if size > limit {
            Err(format!("child output exceeds {limit} bytes"))
        } else {
            Ok(())
        }
    }

    fn read(&self, limit: u64) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        File::open(self.paths.shared())
            .map_err(|error| format!("capture read failed: {error}"))?
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("capture read failed: {error}"))?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests;
