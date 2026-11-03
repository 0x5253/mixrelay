//! Starts, observes and reaps only the isolated child owned by one hosted instance.

use crate::{
    PLUGIN_WORKER_ARGUMENT, PROCESSOR_STATE_TIMEOUT, WORKER_FAILED, WORKER_READY,
    WORKER_START_TIMEOUT, catalog::CatalogEntry, control::HostedProcessorControl,
    shared_transport::ParentAudio,
};

#[cfg(test)]
mod tests;
use std::{
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) fn spawn_processor_worker(
    entry: &CatalogEntry,
    shared_path: &Path,
    config_path: &Path,
) -> Result<Child, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .arg(PLUGIN_WORKER_ARGUMENT)
        .arg(entry.rack.locator.encode()?);
    command
        .arg(shared_path)
        .arg(config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command.spawn().map_err(|error| error.to_string())
}

#[derive(Clone, Copy)]
pub(super) struct Timeouts {
    pub(super) startup: Duration,
    pub(super) native_call: Duration,
    pub(super) shutdown: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            startup: WORKER_START_TIMEOUT,
            native_call: PROCESSOR_STATE_TIMEOUT,
            shutdown: Duration::from_secs(2),
        }
    }
}

/// Owns the parent's lifetime pipe and the monitor that must reap its exact child.
pub(super) struct WorkerProcess {
    stdin: Option<ChildStdin>,
    monitor: WorkerMonitor,
}

impl WorkerProcess {
    pub(super) fn start(
        mut child: Child,
        control: Arc<HostedProcessorControl>,
        shared: &ParentAudio,
        timeouts: Timeouts,
    ) -> Result<Self, String> {
        let stdin = child.stdin.take();
        if let Err(error) = wait_for_worker(&mut child, shared, timeouts.startup) {
            drop(stdin);
            kill_and_reap(&mut child);
            return Err(error);
        }
        let monitor = WorkerMonitor::spawn(child, control, timeouts)?;
        Ok(Self { stdin, monitor })
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        drop(self.stdin.take());
        self.monitor.stop_and_join();
    }
}

pub(super) struct WorkerMonitor {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl WorkerMonitor {
    /// Transfers one owned child to its supervisor, or kills and reaps it if spawning fails.
    pub(super) fn spawn(
        child: Child,
        control: Arc<HostedProcessorControl>,
        timeouts: Timeouts,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let thread_child = Arc::clone(&child_slot);
        let thread = thread::Builder::new()
            .name("mixrelay-processor-monitor".to_owned())
            .spawn(move || {
                let child = thread_child
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(child) = child {
                    monitor_child(child, &control, &thread_stop, timeouts);
                }
            });
        match thread {
            Ok(thread) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Err(error) => {
                // A rejected spawn leaves the child in the slot; no unowned process escapes.
                if let Some(mut child) = child_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    kill_and_reap(&mut child);
                }
                Err(error.to_string())
            }
        }
    }

    pub(super) fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WorkerMonitor {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn monitor_child(
    mut child: Child,
    control: &HostedProcessorControl,
    stop: &AtomicBool,
    timeouts: Timeouts,
) {
    use crate::supervision::{Action, ChildState, Failure, StopStatus, Supervisor};
    let mut supervisor =
        Supervisor::new(timeouts.native_call, timeouts.shutdown, control.clock.now());
    let mut faults = crate::process_fault::FaultObserver::default();
    loop {
        report_process_faults(control, &mut faults);
        let now = control.clock.now();
        if supervisor.sample_due(now) {
            control.timing.request();
        }
        let status = child.try_wait();
        let child_state = match &status {
            Ok(Some(_)) => ChildState::Exited,
            Ok(None) => ChildState::Running,
            Err(_) => ChildState::WaitFailed,
        };
        let activity = native_activity(control);
        let stop_status = if stop.load(Ordering::Acquire) {
            StopStatus::Requested
        } else {
            StopStatus::Running
        };
        match supervisor.observe(child_state, stop_status, activity, now) {
            Action::Finished { unexpected } => {
                report_process_faults(control, &mut faults);
                if unexpected {
                    fail_worker(control, child.id(), "native_child_exit", activity.phase());
                    if let Ok(Some(status)) = status {
                        tracing::warn!(%status, "native child exit status");
                    }
                }
                return;
            }
            Action::Kill { failure } => {
                if let Some(failure) = failure {
                    let (cause, phase) = match failure {
                        Failure::ChildWait => ("child_wait_failed", activity.phase()),
                        Failure::NativeTimeout(stage) => ("native_operation_timeout", stage.name()),
                    };
                    fail_worker(control, child.id(), cause, phase);
                }
                kill_and_reap(&mut child);
                if matches!(failure, Some(Failure::NativeTimeout(_))) {
                    report_process_faults(control, &mut faults);
                }
                return;
            }
            Action::Continue => {}
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn native_activity(control: &HostedProcessorControl) -> crate::supervision::NativeActivity {
    let header = control.shared.header();
    crate::supervision::NativeActivity {
        creator: header.native_operation.snapshot(),
        dsp: header.dsp_operation.snapshot(),
        parent_request_pending: header.editor_sequence.load(Ordering::Acquire)
            != header.editor_consumed.load(Ordering::Acquire)
            || control.shared.has_pending_request(),
    }
}

fn fail_worker(
    control: &HostedProcessorControl,
    pid: u32,
    cause: &'static str,
    phase: &'static str,
) {
    let header = control.shared.header();
    header.worker_status.store(WORKER_FAILED, Ordering::Release);
    header.shutdown.store(1, Ordering::Release);
    crate::latency::publish_intrinsic_frames(header, None);
    tracing::warn!(
        os = std::env::consts::OS,
        boundary = "native_processor",
        native_format = control.native_format(),
        phase,
        cause,
        pid,
        instance_id = header.instance_id,
        "isolated native worker failed"
    );
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn report_process_faults(
    control: &HostedProcessorControl,
    observer: &mut crate::process_fault::FaultObserver,
) {
    let header = control.shared.header();
    if let Some(fault) = observer.native_failure(&header.process_faults) {
        let native_code = match fault {
            truce_rack_core::ProcessFault::NativeStatus(code) => Some(code),
            _ => None,
        };
        tracing::warn!(
            os = std::env::consts::OS,
            boundary = "native_processor",
            native_format = control.native_format(),
            phase = "process",
            cause = "first_native_process_failure",
            ?fault,
            native_code,
            instance_id = header.instance_id,
            "hosted processor native failure detail"
        );
    }
    for delta in observer
        .observe(&header.process_faults)
        .into_iter()
        .flatten()
    {
        tracing::warn!(
            os = std::env::consts::OS,
            boundary = "native_processor",
            native_format = control.native_format(),
            phase = delta.kind.phase(),
            cause = delta.kind.cause(),
            instance_id = header.instance_id,
            new_failures = delta.new_failures,
            failures = delta.total,
            "hosted processor rejected native work"
        );
    }
}

pub(super) fn cleanup_worker_paths(shared_path: &Path, config_path: &Path) {
    let _ = std::fs::remove_file(shared_path);
    cleanup_worker_config_path(config_path);
}

pub(super) fn cleanup_worker_config_path(config_path: &Path) {
    let _ = std::fs::remove_file(config_path);
}

fn wait_for_worker(
    child: &mut Child,
    shared: &ParentAudio,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match shared.header().worker_status.load(Ordering::Acquire) {
            WORKER_READY => return Ok(()),
            WORKER_FAILED => return Err("processor worker initialization failed".to_owned()),
            _ => {}
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            let phase = shared.header().native_operation.snapshot().phase();
            return Err(format!("processor worker exited during {phase}: {status}"));
        }
        if Instant::now() >= deadline {
            let phase = shared.header().native_operation.snapshot().phase();
            return Err(format!("processor worker timed out during {phase}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}
