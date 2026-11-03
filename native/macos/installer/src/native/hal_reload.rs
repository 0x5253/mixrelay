//! Restarts only hosts mapping the installed, previously verified `MixRelay` HAL.

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use super::{
    NativeError,
    launchctl::{self, ChildOutput},
};

const PS: &str = "/bin/ps";
const PGREP: &str = "/usr/bin/pgrep";
/// Exact (`-x`) extended regular expression for the host's full arguments.
const HOST_PATTERN: &str = r"Core Audio Driver \(MixRelayHAL\.driver\)";
const LSOF: &str = "/usr/sbin/lsof";
const KILL: &str = "/bin/kill";
const HOST_NAME: &str = "Core Audio Driver (MixRelayHAL.driver)";
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReloadStrategy {
    TargetedHost,
    FullCoreAudio,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostIdentity {
    pub(super) started: String,
    pub(super) command: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CapturedHosts {
    hosts: Vec<(u32, HostIdentity)>,
}

pub(super) trait HostProcesses {
    fn candidates(&mut self) -> Result<Vec<u32>, NativeError>;
    // The start time is retained as well as checking comm and the exact mapping.
    fn identity(&mut self, pid: u32, bundles: &[PathBuf]) -> Result<HostIdentity, NativeError>;
    fn signal(&mut self, pid: u32, signal: &'static str) -> Result<(), NativeError>;
    fn wait_for_exit(&mut self, pid: u32) -> Result<bool, NativeError>;
}

/// Capture before commit (or before file rollback), never after the move.
/// Uncertain observations disable targeted reload without preventing full reload.
pub(super) fn capture(bundles: &[PathBuf]) -> Option<CapturedHosts> {
    match capture_hosts(&mut CommandHosts(launchctl::run_command), bundles) {
        Ok(hosts) => Some(hosts),
        Err(error) => {
            tracing::warn!(%error, "HAL host snapshot unavailable; full CoreAudio reload required");
            None
        }
    }
}

pub(super) fn capture_hosts(
    processes: &mut impl HostProcesses,
    bundles: &[PathBuf],
) -> Result<CapturedHosts, NativeError> {
    let pids = processes.candidates()?;
    if pids.is_empty() {
        return Err(failure("no MixRelay HAL host is loaded"));
    }
    let hosts = pids
        .into_iter()
        .map(|pid| {
            processes
                .identity(pid, bundles)
                .map(|identity| (pid, identity))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CapturedHosts { hosts })
}

pub(super) fn reload_and_wait(
    captured: Option<&CapturedHosts>,
    bundles: &[PathBuf],
    mut ready: impl FnMut() -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    let mut processes = CommandHosts(launchctl::run_command);
    reload_with_readiness(
        captured,
        bundles,
        &mut processes,
        launchctl::reload_coreaudio,
        &mut ready,
    )
}

pub(super) fn reload_with_readiness(
    captured: Option<&CapturedHosts>,
    bundles: &[PathBuf],
    processes: &mut impl HostProcesses,
    full_reload: impl FnOnce() -> Result<(), NativeError>,
    mut ready: impl FnMut() -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    if let Some(captured) = captured {
        match restart_hosts(processes, captured, bundles) {
            Ok(()) => {
                tracing::info!(strategy = ?ReloadStrategy::TargetedHost, "MixRelay HAL reload completed");
                tracing::info!("Checking readiness after HAL reload");
                match ready() {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        tracing::warn!(%error, "Targeted HAL reload readiness failed; falling back to full CoreAudio reload");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "Targeted HAL reload unavailable; falling back to full CoreAudio reload");
            }
        }
    }
    full_reload()?;
    tracing::info!("Checking readiness after HAL reload");
    ready()
}

fn restart_hosts(
    processes: &mut impl HostProcesses,
    captured: &CapturedHosts,
    bundles: &[PathBuf],
) -> Result<(), NativeError> {
    let pids = processes.candidates()?;
    if pids.len() != captured.hosts.len()
        || captured.hosts.iter().any(|(pid, _)| !pids.contains(pid))
    {
        return Err(failure("MixRelay HAL host set changed after capture"));
    }
    // Revalidate the entire captured set before any host can be signalled.
    for (pid, identity) in &captured.hosts {
        verify_identity(processes, *pid, identity, bundles)?;
    }
    for (pid, identity) in &captured.hosts {
        signal_verified(processes, *pid, identity, bundles, "-TERM")?;
        if !processes.wait_for_exit(*pid)? {
            signal_verified(processes, *pid, identity, bundles, "-KILL")?;
            if !processes.wait_for_exit(*pid)? {
                return Err(failure("MixRelay HAL host termination was not confirmed"));
            }
        }
    }
    Ok(())
}

fn signal_verified(
    processes: &mut impl HostProcesses,
    pid: u32,
    identity: &HostIdentity,
    bundles: &[PathBuf],
    signal: &'static str,
) -> Result<(), NativeError> {
    verify_identity(processes, pid, identity, bundles)?;
    processes.signal(pid, signal)
}

fn verify_identity(
    processes: &mut impl HostProcesses,
    pid: u32,
    identity: &HostIdentity,
    bundles: &[PathBuf],
) -> Result<(), NativeError> {
    if &processes.identity(pid, bundles)? != identity {
        return Err(failure(
            "MixRelay HAL host PID was reused before signalling",
        ));
    }
    Ok(())
}

struct CommandHosts<F>(F);

impl<F> HostProcesses for CommandHosts<F>
where
    F: FnMut(&'static str, &[&str]) -> Result<ChildOutput, NativeError>,
{
    fn candidates(&mut self) -> Result<Vec<u32>, NativeError> {
        // Only matching PIDs are printed: the whole process table of an ordinary Mac
        // exceeds the bounded command output. Each candidate is still validated by
        // its exact command, start time and mapped executable.
        let output = (self.0)(PGREP, &["-x", "-f", HOST_PATTERN])?;
        if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
            return Ok(Vec::new());
        }
        let mut pids = Vec::new();
        for line in successful_text(&output)?.lines() {
            let pid = parse_pid(line.trim())?;
            if pids.contains(&pid) {
                return Err(failure("duplicate host discovery PID"));
            }
            pids.push(pid);
        }
        Ok(pids)
    }

    fn identity(&mut self, pid: u32, bundles: &[PathBuf]) -> Result<HostIdentity, NativeError> {
        let pid = pid.to_string();
        let output = (self.0)(PS, &["-p", &pid, "-o", "lstart=,comm="])?;
        let text = successful_text(&output)?
            .trim_start()
            .trim_end_matches('\n');
        // C-locale ps lstart is a fixed 24-byte timestamp. Read it and comm
        // together so the generation and name come from the same observation.
        let (started, name) = text
            .split_at_checked(24)
            .ok_or_else(|| failure("malformed host identity output"))?;
        if name.trim_start() != HOST_NAME || started.trim().is_empty() {
            return Err(failure("MixRelay HAL host process identity mismatch"));
        }
        // Mapped executables only (`txt`): the HAL binary is one, and the full file
        // table of a busy host could exceed the bounded command output.
        let mapping = (self.0)(LSOF, &["-a", "-p", &pid, "-d", "txt", "-Fn"])?;
        if !successful_text(&mapping)?.lines().any(|line| {
            bundles.iter().any(|bundle| {
                line == format!("n{}", bundle.join("Contents/MacOS/MixRelayHAL").display())
            })
        }) {
            return Err(failure("MixRelay HAL host executable mapping mismatch"));
        }
        Ok(HostIdentity {
            started: started.to_owned(),
            command: name.trim_start().to_owned(),
        })
    }

    fn signal(&mut self, pid: u32, signal: &'static str) -> Result<(), NativeError> {
        let output = (self.0)(KILL, &[signal, &pid.to_string()])?;
        if !successful_text(&output)?.is_empty() {
            return Err(failure("unexpected signal command output"));
        }
        Ok(())
    }

    fn wait_for_exit(&mut self, pid: u32) -> Result<bool, NativeError> {
        wait_for_exit(&mut self.0, pid, Instant::now, thread::sleep)
    }
}

fn wait_for_exit(
    command: &mut impl FnMut(&'static str, &[&str]) -> Result<ChildOutput, NativeError>,
    pid: u32,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<bool, NativeError> {
    let deadline = now() + EXIT_TIMEOUT;
    loop {
        let output = command(PS, &["-p", &pid.to_string(), "-o", "pid="])?;
        if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
            return Ok(true);
        }
        if parse_pid(successful_text(&output)?.trim())? != pid {
            return Err(failure("unexpected PID in exit confirmation"));
        }
        if now() >= deadline {
            return Ok(false);
        }
        sleep(Duration::from_millis(100));
    }
}

fn successful_text(output: &ChildOutput) -> Result<&str, NativeError> {
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(failure(
            "HAL host command failed or returned unexpected stderr",
        ));
    }
    std::str::from_utf8(&output.stdout).map_err(|_| failure("HAL host command output is not UTF-8"))
}

fn parse_pid(text: &str) -> Result<u32, NativeError> {
    text.parse::<u32>()
        .ok()
        .filter(|pid| *pid > 1 && *pid <= i32::MAX.cast_unsigned())
        .ok_or_else(|| failure("invalid host PID"))
}

fn failure(message: &str) -> NativeError {
    NativeError::Activation(message.to_owned())
}

#[cfg(test)]
mod tests;
