//! Owned native test processes must not outlive a failed Cargo test. Nonblocking
//! pipes keep both diagnostics and the deadline bounded, including noisy children.
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(test)]
mod tests;

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // SAFETY: process_group(0) creates a group owned by this child. A
        // negative PID targets only that group, including the child's own
        // descendants. Always reap our direct child, even after an I/O failure.
        if let Ok(pid) = i32::try_from(self.0.id()) {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: the borrowed pipe owns a valid descriptor for both calls. We only
    // add O_NONBLOCK and preserve every other descriptor status flag.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Reads at most eight chunks, so a continuously writing child cannot starve the deadline
/// check. Returns the bytes read, or `None` once every writer closed the pipe.
fn drain(pipe: &mut impl Read, output: &mut Vec<u8>) -> io::Result<Option<usize>> {
    let mut chunk = [0; 8192];
    let mut read = 0;
    for _ in 0..8 {
        match pipe.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                if output.len() + count > OUTPUT_LIMIT {
                    return Err(io::Error::other(
                        "native test diagnostic output exceeded 1 MiB",
                    ));
                }
                output.extend_from_slice(&chunk[..count]);
                read += count;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Some(read))
}

/// Waits at most `interval` for output on the pipes still open. Waking on readiness rather
/// than sleeping keeps throughput independent of the kernel's pipe buffer size.
fn await_output(pipes: &[(&dyn AsRawFd, Option<usize>)], interval: Duration) -> io::Result<()> {
    let mut open: Vec<_> = pipes
        .iter()
        .filter(|(_, read)| read.is_some())
        .map(|(pipe, _)| libc::pollfd {
            fd: pipe.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    if open.is_empty() {
        thread::sleep(interval);
        return Ok(());
    }
    let count = libc::nfds_t::try_from(open.len()).map_err(io::Error::other)?;
    let milliseconds = i32::try_from(interval.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: `open` is an exclusively borrowed array of `count` initialized entries.
    if unsafe { libc::poll(open.as_mut_ptr(), count, milliseconds) } < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

/// Decides when a run exceeded its deadline. Production measures elapsed time from the
/// start of collection; a test can instead expire it on the child state it observed.
trait Deadline {
    /// Whether the run must end now, given the child's stdout so far.
    fn expired(&mut self, stdout: &[u8]) -> bool;
    /// How long to wait for output before deciding again.
    fn wait(&self) -> Duration;
}

/// Expires once `timeout` elapsed since collection started; `Duration::MAX` never does.
struct Elapsed {
    started: Instant,
    timeout: Duration,
}

impl Deadline for Elapsed {
    fn expired(&mut self, _: &[u8]) -> bool {
        self.started.elapsed() >= self.timeout
    }
    fn wait(&self) -> Duration {
        POLL_INTERVAL.min(self.timeout.saturating_sub(self.started.elapsed()))
    }
}

/// Collects both pipes until the child exits or `deadline` expires.
fn collect(
    child: &mut Child,
    (stdout_pipe, stderr_pipe): (&mut ChildStdout, &mut ChildStderr),
    (stdout, stderr): (&mut Vec<u8>, &mut Vec<u8>),
    deadline: &mut impl Deadline,
) -> io::Result<ExitStatus> {
    nonblocking(stdout_pipe)?;
    nonblocking(stderr_pipe)?;
    loop {
        let read = (drain(stdout_pipe, stdout)?, drain(stderr_pipe, stderr)?);
        if let Some(status) = child.try_wait()? {
            // The child has exited: drain the bounded remaining pipe data.
            // Descendants holding a pipe open cannot stall nonblocking reads.
            for _ in 0..=(OUTPUT_LIMIT / 65536) {
                drain(stdout_pipe, stdout)?;
                drain(stderr_pipe, stderr)?;
            }
            return Ok(status);
        }
        if deadline.expired(stdout) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "native test deadline exceeded",
            ));
        }
        if !matches!(read, (Some(1..), _) | (_, Some(1..))) {
            await_output(
                &[(&*stdout_pipe, read.0), (&*stderr_pipe, read.1)],
                deadline.wait(),
            )?;
        }
    }
}

/// Runs `command` in its own process group, collecting its output until it exits or
/// `timeout` elapses. Every process of the group is killed and the child reaped either way.
pub(super) fn run(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    run_until(command, || Elapsed {
        started: Instant::now(),
        timeout,
    })
}

/// [`run`] with the deadline `start` returns once the child launched.
fn run_until<D: Deadline>(
    command: &mut Command,
    start: impl FnOnce() -> D,
) -> Result<Output, String> {
    let label = format!("{command:?}");
    let mut child = OwnedChild(
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot launch {label}: {error}"))?,
    );
    let mut stdout_pipe = child
        .0
        .stdout
        .take()
        .ok_or("requested stdout pipe is missing")?;
    let mut stderr_pipe = child
        .0
        .stderr
        .take()
        .ok_or("requested stderr pipe is missing")?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    match collect(
        &mut child.0,
        (&mut stdout_pipe, &mut stderr_pipe),
        (&mut stdout, &mut stderr),
        &mut start(),
    ) {
        Ok(status) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        Err(error) => Err(format!(
            "{label}: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        )),
    }
}
