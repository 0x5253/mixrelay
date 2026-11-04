//! Runs fixed native programs with a clean environment, bounded output and a deadline.

use std::{
    io::{self, Read},
    process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::NativeError;

const MAX_CHILD_OUTPUT: usize = 64 * 1024;
const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DIAGNOSTIC_BYTES: usize = 2048;

/// Exit status and complete, within-budget output of one reaped child.
pub(super) struct ChildOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

/// Runs a state-changing command that must succeed silently.
pub(super) fn run_mutation(program: &'static str, arguments: &[&str]) -> Result<(), NativeError> {
    let output = run_command(program, arguments)?;
    if output.status.success() && output.stdout.is_empty() && output.stderr.is_empty() {
        Ok(())
    } else {
        Err(command_result(
            program,
            output.status,
            &output.stdout,
            &output.stderr,
        ))
    }
}

/// Runs `program` and collects both streams; a timeout kills and reaps the child.
pub(super) fn run_command(
    program: &'static str,
    arguments: &[&str],
) -> Result<ChildOutput, NativeError> {
    let mut owner = ChildOwner(spawn(program, arguments)?);
    let (stdout, stderr) = take_pipes(program, &mut owner.0)?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_CHILD_OUTPUT));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_CHILD_OUTPUT));
    let status = wait_bounded(program, &mut owner.0);
    // A collector panic outranks the exit status; stream read errors follow it.
    let stdout = join_collector(program, &status, "stdout", stdout_reader)?;
    let stderr = join_collector(program, &status, "stderr", stderr_reader)?;
    let status = status?;
    let (stdout, stderr) = (stdout?, stderr?);
    if stdout.truncated || stderr.truncated {
        return Err(NativeError::CommandResult {
            program,
            status: status_string(status),
            detail: "command exceeded the bounded output budget".to_owned(),
        });
    }
    Ok(ChildOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn spawn(program: &'static str, arguments: &[&str]) -> Result<Child, NativeError> {
    Command::new(program)
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| NativeError::CommandIo { program, source })
}

fn take_pipes(
    program: &'static str,
    child: &mut Child,
) -> Result<(ChildStdout, ChildStderr), NativeError> {
    let missing = |stream: &str| NativeError::CommandIo {
        program,
        source: io::Error::other(format!("{stream} pipe was not created")),
    };
    let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    Ok((stdout, stderr))
}

/// Joins one stream collector. The outer error is a collector panic; the inner one a read failure.
fn join_collector(
    program: &'static str,
    status: &Result<ExitStatus, NativeError>,
    stream: &str,
    collector: JoinHandle<io::Result<BoundedBytes>>,
) -> Result<Result<BoundedBytes, NativeError>, NativeError> {
    let collected = collector.join().map_err(|_| NativeError::CommandResult {
        program,
        status: status
            .as_ref()
            .map_or_else(|_| "unknown".to_owned(), |status| status_string(*status)),
        detail: format!("{stream} collection thread panicked"),
    })?;
    Ok(collected.map_err(|source| NativeError::CommandIo { program, source }))
}

fn wait_bounded(program: &'static str, child: &mut Child) -> Result<ExitStatus, NativeError> {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|source| NativeError::CommandIo { program, source })?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .map_err(|source| NativeError::CommandIo { program, source })?;
            child
                .wait()
                .map_err(|source| NativeError::CommandIo { program, source })?;
            return Err(NativeError::CommandResult {
                program,
                status: "timeout".to_owned(),
                detail: "command exceeded the ten-second deadline and was reaped".to_owned(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Keeps at most `limit` bytes but drains the rest so the child never blocks on a full pipe.
fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<BoundedBytes> {
    let mut bytes = Vec::with_capacity(limit.min(4096));
    let mut truncated = false;
    let mut chunk = [0; 4096];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let available = limit.saturating_sub(bytes.len());
        let kept = count.min(available);
        bytes.extend_from_slice(&chunk[..kept]);
        truncated |= kept != count;
    }
    Ok(BoundedBytes { bytes, truncated })
}

/// Kills and reaps a child that is still running when its owner is dropped.
struct ChildOwner(Child);

impl Drop for ChildOwner {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn command_result(
    program: &'static str,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> NativeError {
    NativeError::CommandResult {
        program,
        status: status_string(status),
        detail: output_detail(stdout, stderr),
    }
}

/// Exit code text, or the signal wording used for a child without one.
pub(super) fn exit_code_string(code: Option<i32>) -> String {
    code.map_or_else(
        || "terminated by signal".to_owned(),
        |code| code.to_string(),
    )
}

fn status_string(status: ExitStatus) -> String {
    exit_code_string(status.code())
}

/// Escaped and bounded stdout/stderr text for a diagnostic.
pub(super) fn output_detail(stdout: &[u8], stderr: &[u8]) -> String {
    format!("stdout={}; stderr={}", escaped(stdout), escaped(stderr))
}

fn escaped(bytes: &[u8]) -> String {
    let mut result = String::new();
    for character in String::from_utf8_lossy(bytes).chars() {
        if result.len() >= MAX_DIAGNOSTIC_BYTES {
            result.push('…');
            break;
        }
        result.extend(character.escape_default());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_report_the_program_that_actually_ran() {
        let error = run_mutation("/usr/bin/false", &[]).expect_err("false must fail");
        assert!(matches!(
            error,
            NativeError::CommandResult { program: "/usr/bin/false", status, .. }
                if status == "1"
        ));

        let error = run_command("/nonexistent-mixrelay-command", &[])
            .err()
            .expect("missing command must fail to spawn");
        assert!(matches!(
            error,
            NativeError::CommandIo {
                program: "/nonexistent-mixrelay-command",
                ..
            }
        ));
    }

    #[test]
    fn mutations_that_succeed_but_print_output_are_rejected() {
        let error = run_mutation("/bin/sh", &["-c", "printf unexpected"])
            .expect_err("a mutation must succeed silently");
        assert!(matches!(
            error,
            NativeError::CommandResult { program: "/bin/sh", status, detail }
                if status == "0" && detail.contains("unexpected")
        ));
    }

    #[test]
    fn collection_preserves_stdout_stderr_and_exit_status() {
        let output = run_command(
            "/bin/sh",
            &["-c", "printf 'fixture stdout'; printf 'fixture stderr' >&2"],
        )
        .expect("fixture command succeeds");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"fixture stdout");
        assert_eq!(output.stderr, b"fixture stderr");
    }

    #[test]
    fn collection_drains_output_after_its_budget_is_exhausted() {
        let mut reader = io::Cursor::new(b"12345");
        let output = read_bounded(&mut reader, 3).expect("fixture bytes are readable");
        assert_eq!(output.bytes, b"123");
        assert!(output.truncated);
        assert_eq!(reader.position(), 5);
    }

    #[test]
    fn diagnostics_escape_control_bytes_and_stop_at_their_budget() {
        assert_eq!(
            output_detail(b"a\nb", b"\x1b"),
            "stdout=a\\nb; stderr=\\u{1b}"
        );
        let long = escaped(&[b'x'; MAX_DIAGNOSTIC_BYTES * 2]);
        assert_eq!(long.len(), MAX_DIAGNOSTIC_BYTES + '…'.len_utf8());
        assert!(long.ends_with('…'));
    }
}
