//! Controller and editor child lifetimes remain independent of window ownership.
use std::{
    os::unix::process::CommandExt as _,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) struct ControllerLaunch {
    exited: Arc<AtomicBool>,
}

impl ControllerLaunch {
    pub(super) fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

pub(crate) fn spawn_controller(executable: &std::path::Path) -> std::io::Result<ControllerLaunch> {
    #[cfg(target_os = "macos")]
    if !tauri::is_dev()
        && executable
            == std::path::Path::new(mixrelay_macos_transport::install_contract::APP_EXECUTABLE_PATH)
    {
        let launcher = std::env::current_exe()?
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or_else(|| std::io::Error::other("installed app bundle unavailable"))?
            .join("Resources/mixrelay-driver-launcher");
        return spawn_reaped_controller(&launcher, false);
    }
    spawn_reaped_controller(executable, true)
}

fn spawn_reaped_controller(
    executable: &std::path::Path,
    runtime_argument: bool,
) -> std::io::Result<ControllerLaunch> {
    let mut command = std::process::Command::new(executable);
    if runtime_argument {
        command.arg("--driver-runtime");
    }
    let exited = Arc::new(AtomicBool::new(false));
    let reaper = DetachedReaper::prepare("mixrelay-controller-reaper", Some(Arc::clone(&exited)))?;
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    reaper.start(&mut command)?;
    Ok(ControllerLaunch { exited })
}

pub(crate) fn spawn_editor(executable: &std::path::Path) -> std::io::Result<()> {
    let reaper = DetachedReaper::prepare("mixrelay-editor-reaper", None)?;
    reaper.start(
        std::process::Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
}

/// A prepared worker receives the child only after thread creation has succeeded.
struct DetachedReaper {
    child: std::sync::mpsc::SyncSender<std::process::Child>,
}

impl DetachedReaper {
    fn prepare(name: &str, observed_exit: Option<Arc<AtomicBool>>) -> std::io::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<std::process::Child>(1);
        std::thread::Builder::new().name(name.into()).spawn(move || {
            let Ok(mut child) = receiver.recv() else { return; };
            let pid = child.id();
            match child.wait() {
                Ok(status) => {
                    if let Some(exited) = observed_exit {
                        // Release publishes reaping before a retry observes completion.
                        exited.store(true, Ordering::Release);
                    }
                    tracing::info!(pid, code = status.code(), "launched desktop process exited");
                }
                Err(error) => tracing::warn!(%error, pid, "could not observe launched desktop process exit"),
            }
        })?;
        Ok(Self { child: sender })
    }

    fn start(self, command: &mut std::process::Command) -> std::io::Result<()> {
        let child = command.spawn()?;
        if let Err(std::sync::mpsc::SendError(mut child)) = self.child.send(child) {
            // A failed handoff still owns this child; do not leave it unreaped.
            let killed = child.kill();
            let waited = child.wait();
            waited?;
            killed?;
            return Err(std::io::Error::other(
                "desktop process reaper stopped before handoff",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
