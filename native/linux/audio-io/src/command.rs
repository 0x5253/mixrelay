//! Bounded native discovery. Nonblocking pipes avoid both full-pipe deadlocks and
//! reader threads surviving a timed-out command. Only the owned child is killed.
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub(super) fn output(program: &str, args: &[&str]) -> io::Result<Output> {
    bounded_output(program, args, Duration::from_secs(2))
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: live owned descriptor, only descriptor status flags are changed.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: preserve existing flags, set nonblocking on this pipe descriptor.
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut chunk = [0; 8192];
    // Even a continuously writing child cannot postpone the outer deadline.
    for _ in 0..64 {
        match pipe.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if bytes.len() + count > limit {
                    return Err(io::Error::other("native discovery output limit exceeded"));
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub(super) fn bounded_output(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<Output> {
    let started = Instant::now();
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "native discovery timed out",
        ));
    }
    let mut child = OwnedChild(
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let mut stderr = child
        .0
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr"))?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    loop {
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "native discovery timed out",
            ));
        }
        drain(&mut stdout, &mut out, 16 * 1024 * 1024)?;
        drain(&mut stderr, &mut err, 256 * 1024)?;
        if let Some(status) = child.0.try_wait()? {
            let out_done = drain(&mut stdout, &mut out, 16 * 1024 * 1024)?;
            let err_done = drain(&mut stderr, &mut err, 256 * 1024)?;
            if out_done && err_done {
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(all(test, feature = "native-lab"))]
mod tests {
    use super::*;
    #[test]
    fn timeout_and_oversized_output_are_bounded_without_reader_threads() {
        let started = Instant::now();
        assert_eq!(
            bounded_output("/bin/sleep", &["2"], Duration::from_millis(20))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        let result = bounded_output("/bin/echo", &["registry"], Duration::from_secs(1)).unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"registry\n");
        assert!(drain(&mut &b"12345"[..], &mut Vec::new(), 4).is_err());
    }
    #[test]
    fn large_native_output_is_drained_without_pipe_deadlock_or_truncation() {
        let output = bounded_output(
            "/bin/dd",
            &["if=/dev/zero", "bs=1048576", "count=2"],
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 2 * 1024 * 1024);
        assert!(output.stdout.iter().all(|byte| *byte == 0));
    }
}
