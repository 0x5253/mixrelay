//! Owns the engine child and its private shutdown lease.
use std::{
    io,
    path::Path,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
};

use mixrelay_time::{MonotonicClock, Sleeper};

mod readiness;
mod reaping;
use reaping::Reaper;

/// A controller-owned engine process and its private shutdown lease.
///
/// The lease is the only object used to request an orderly engine shutdown:
/// closing it closes the child's standard-input pipe. The child handle remains
/// owned here until the engine has exited. A reaper thread is prepared before
/// spawning the child and receives ownership when this value is dropped.
#[derive(Debug)]
pub struct ControllerEngine {
    owned: Option<OwnedChild>,
    lease: Option<ChildStdin>,
    id: u32,
}

#[derive(Debug)]
struct OwnedChild {
    child: Child,
    reaper: Reaper,
    reaped: bool,
}

impl ControllerEngine {
    /// Spawns `executable` in controller-owned mode and waits for its readiness
    /// acknowledgement on the child's private standard-output pipe.
    ///
    /// The executable is invoked with exactly [`crate::CONTROLLER_ARGUMENT`]. Its
    /// standard input and output are piped, while standard error is inherited.
    /// Startup fails if the acknowledgement is not received within 30 seconds,
    /// is malformed, or the child exits before the startup check completes.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if reaper preparation, spawning, piping, readiness, or
    /// the startup liveness check fails. On every post-spawn error the private
    /// lease is closed, the owned child is given a bounded grace period measured on
    /// `clock`, and only that child is killed and reaped if it remains alive.
    pub fn start(executable: &Path, clock: &(impl MonotonicClock + Sleeper)) -> io::Result<Self> {
        Self::start_with_reaper(executable, clock, Reaper::prepare)
    }

    fn start_with_reaper(
        executable: &Path,
        clock: &(impl MonotonicClock + Sleeper),
        prepare: impl FnOnce() -> io::Result<Reaper>,
    ) -> io::Result<Self> {
        let reaper = prepare().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("prepare owned engine reaper: {error}"),
            )
        })?;
        let mut command = Command::new(executable);
        command
            .arg(crate::CONTROLLER_ARGUMENT)
            // The controller reaches its engine at the session's socket directory, so the
            // child must not follow a socket-directory override (MIXRELAY_IPC_DIR) it
            // happened to inherit.
            .env_remove("MIXRELAY_IPC_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = command.spawn()?;
        let id = child.id();
        let Some(lease) = child.stdin.take() else {
            return Err(fail_start(
                child,
                reaper,
                clock,
                io::Error::other("controller-owned engine stdin pipe was not created"),
            ));
        };
        let Some(stdout) = child.stdout.take() else {
            drop(lease);
            return Err(fail_start(
                child,
                reaper,
                clock,
                io::Error::other("controller-owned engine stdout pipe was not created"),
            ));
        };

        if let Err(error) = readiness::wait(stdout) {
            drop(lease);
            return Err(fail_start(child, reaper, clock, error));
        }

        if let Err(error) = reject_child_that_exited_during_startup(&mut child) {
            drop(lease);
            return Err(fail_start(child, reaper, clock, error));
        }

        Ok(Self {
            owned: Some(OwnedChild {
                child,
                reaper,
                reaped: false,
            }),
            lease: Some(lease),
            id,
        })
    }

    /// Returns the operating-system process identifier of the owned child.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Closes the private lease and requests the engine's existing graceful
    /// drain. This does not kill the child or wait for it to exit.
    pub fn request_shutdown(&mut self) {
        self.lease.take();
    }

    /// Polls the owned child without forcing termination.
    ///
    /// `Ok(None)` means that the process is still running. Callers that own an
    /// async UI should poll this method from their async shutdown state machine
    /// until it returns `Ok(Some(status))`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the operating system cannot inspect the child.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(owned) = &mut self.owned else {
            return Ok(None);
        };
        let result = owned.child.try_wait();
        if matches!(result, Ok(Some(_))) {
            owned.reaped = true;
        }
        result
    }
}

impl Drop for ControllerEngine {
    fn drop(&mut self) {
        self.lease.take();
        if let Some(OwnedChild {
            child,
            reaper,
            reaped,
        }) = self.owned.take()
            && !reaped
        {
            reaper.schedule(child);
        }
    }
}

fn reject_child_that_exited_during_startup(child: &mut Child) -> io::Result<()> {
    if let Some(status) = child.try_wait()? {
        return Err(io::Error::other(format!(
            "controller-owned engine exited during startup: {status}"
        )));
    }
    Ok(())
}

fn fail_start(
    mut child: Child,
    reaper: Reaper,
    clock: &(impl MonotonicClock + Sleeper),
    error: io::Error,
) -> io::Error {
    // Closing stdin is the first cleanup action so a cooperative child can
    // leave its normal drain path before the bounded grace period expires.
    child.stdin.take();
    match reaping::after_startup_failure(child, reaper, clock) {
        Ok(()) => error,
        Err(cleanup_error) => io::Error::new(
            error.kind(),
            format!("{error}; startup cleanup failed: {cleanup_error}"),
        ),
    }
}

#[cfg(test)]
mod tests;
