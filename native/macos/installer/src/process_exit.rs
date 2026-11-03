//! One answer to "has this process exited?" for every installer wait.
//!
//! `NSRunningApplication.isTerminated` only changes when the observing process
//! receives `LaunchServices` notifications, which a command-line installer may never
//! get, so it can report a quit app as running until a timeout. A PID alone can be
//! reused. A process is identified by its kernel audit token (PID and PID version),
//! read for processes of the installing user; it has exited once its PID no longer
//! yields that token. Signals go through the token, so they can only reach that
//! exact process.

use std::{
    ffi::c_int,
    io,
    time::{Duration, Instant},
};

type MachPort = u32;

/// `<mach/task_info.h>`: `TASK_AUDIT_TOKEN`, eight natural words.
const TASK_AUDIT_TOKEN: c_int = 15;
const TASK_AUDIT_TOKEN_COUNT: u32 = 8;
/// `<bsm/audit.h>`: audit token word holding the PID and its version.
const TOKEN_PID: usize = 5;
const TOKEN_PID_VERSION: usize = 7;

unsafe extern "C" {
    static mach_task_self_: MachPort;
    fn task_name_for_pid(target: MachPort, pid: c_int, task: *mut MachPort) -> c_int;
    fn task_info(task: MachPort, flavor: c_int, info: *mut u32, count: *mut u32) -> c_int;
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> c_int;
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_signal_with_audittoken(token: *mut [u32; 8], signal: c_int) -> c_int;
}

/// A process captured by its kernel audit token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    token: [u32; 8],
}

/// The current audit token for `pid`, or `None` when no process of this user has it.
///
/// A task name port is available for the installing user's own processes. A PID that
/// no longer exists, a zombie, or a replacement owned by another user yields none.
/// Every target is captured while readable, so an unreadable PID is not that target.
fn token_for(pid: i32) -> io::Result<Option<[u32; 8]>> {
    if pid <= 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid PID"));
    }
    let mut task: MachPort = 0;
    // SAFETY: `mach_task_self_` is this task's immortal self port; `task` is writable.
    let result = unsafe { task_name_for_pid(mach_task_self_, pid, &raw mut task) };
    if result != 0 {
        return Ok(None);
    }
    let mut token = [0_u32; 8];
    let mut count = TASK_AUDIT_TOKEN_COUNT;
    // SAFETY: `token` holds exactly `count` natural words for TASK_AUDIT_TOKEN.
    let info = unsafe { task_info(task, TASK_AUDIT_TOKEN, token.as_mut_ptr(), &raw mut count) };
    // SAFETY: `task` was returned to this task above and is released exactly once.
    unsafe { mach_port_deallocate(mach_task_self_, task) };
    if info != 0 || count != TASK_AUDIT_TOKEN_COUNT {
        // The task ended between the two calls.
        return Ok(None);
    }
    if i32::try_from(token[TOKEN_PID]).ok() != Some(pid) {
        return Err(io::Error::other("audit token names another PID"));
    }
    Ok(Some(token))
}

impl ProcessIdentity {
    /// Captures a running process of this user. `Ok(None)` means it has already
    /// exited (or is not this user's process).
    ///
    /// # Errors
    /// Returns invalid-PID or inconsistent kernel answers.
    pub fn capture(pid: i32) -> io::Result<Option<Self>> {
        Ok(token_for(pid)?.map(|token| Self { token }))
    }

    /// The process ID this identity was captured for.
    #[must_use]
    pub fn pid(&self) -> i32 {
        i32::try_from(self.token[TOKEN_PID]).unwrap_or(-1)
    }

    /// Whether the captured process has exited; a reused PID counts as exited.
    ///
    /// # Errors
    /// Returns inconsistent kernel answers.
    pub fn has_exited(&self) -> io::Result<bool> {
        Ok(token_for(self.pid())?
            .is_none_or(|token| token[TOKEN_PID_VERSION] != self.token[TOKEN_PID_VERSION]))
    }

    /// Sends `SIGTERM` to exactly this process; one that already exited is not an
    /// error. The kernel matches the whole token, so a reused PID is never signalled.
    ///
    /// # Errors
    /// Returns signal errors for a process that is still running.
    pub fn terminate(&self) -> io::Result<()> {
        let mut token = self.token;
        // SAFETY: `token` is a complete audit token owned by this frame.
        let result = unsafe { proc_signal_with_audittoken(&raw mut token, libc::SIGTERM) };
        if result == 0 || self.has_exited()? {
            return Ok(());
        }
        Err(io::Error::from_raw_os_error(result))
    }
}

/// Waits until every process has exited or the deadline passes, calling `pump`
/// between checks (for example to run an event loop). Exited processes are not
/// checked again. Returns those still running.
///
/// # Errors
/// Returns inconsistent kernel answers.
pub fn wait_for_exit(
    processes: &[ProcessIdentity],
    deadline: Instant,
    mut pump: impl FnMut(Duration),
) -> io::Result<Vec<ProcessIdentity>> {
    let mut running = processes.to_vec();
    loop {
        let mut still = Vec::with_capacity(running.len());
        for process in running {
            if !process.has_exited()? {
                still.push(process);
            }
        }
        running = still;
        if running.is_empty() || Instant::now() >= deadline {
            return Ok(running);
        }
        pump(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn sleeper() -> (std::process::Child, ProcessIdentity) {
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let identity = ProcessIdentity::capture(pid).unwrap().unwrap();
        assert_eq!(identity.pid(), pid);
        (child, identity)
    }

    #[test]
    fn a_live_process_is_running_and_its_exit_is_observed() {
        let (mut child, identity) = sleeper();
        assert!(!identity.has_exited().unwrap());
        let still = wait_for_exit(
            &[identity],
            Instant::now() + Duration::from_millis(100),
            std::thread::sleep,
        )
        .unwrap();
        assert_eq!(still, [identity]);
        identity.terminate().unwrap();
        // Unreaped, the child is a zombie with no task: already exited.
        assert!(
            wait_for_exit(
                &[identity],
                Instant::now() + Duration::from_secs(5),
                std::thread::sleep
            )
            .unwrap()
            .is_empty()
        );
        child.wait().unwrap();
        assert!(identity.has_exited().unwrap());
        assert_eq!(ProcessIdentity::capture(identity.pid()).unwrap(), None);
        // Terminating an exited process is a no-op, not an error.
        identity.terminate().unwrap();
    }

    #[test]
    fn a_stale_identity_for_a_reused_pid_neither_matches_nor_signals_it() {
        let (mut child, identity) = sleeper();
        let mut token = identity.token;
        token[TOKEN_PID_VERSION] = token[TOKEN_PID_VERSION].wrapping_add(1);
        let stale = ProcessIdentity { token };
        assert!(stale.has_exited().unwrap());
        // The kernel rejects the mismatched token, so the live process survives.
        stale.terminate().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!identity.has_exited().unwrap());
        assert!(child.try_wait().unwrap().is_none());
        identity.terminate().unwrap();
        child.wait().unwrap();
        assert!(ProcessIdentity::capture(0).is_err());
        assert!(ProcessIdentity::capture(-1).is_err());
    }
}
