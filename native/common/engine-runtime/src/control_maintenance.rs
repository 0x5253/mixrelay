//! Joined periodic maintenance worker for supplied engine policy.
use mixrelay_control_protocol::ProtocolError;
use mixrelay_engine::maintenance::CloseLedger;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Policy target retained until the final joined checkpoint completes.
pub trait MaintenanceTarget: Send + Sync {
    fn maintain_control(&self, ledger: &mut CloseLedger);
    /// # Errors
    /// Reports a final native checkpoint failure after maintenance stops.
    fn finalize_control(&self) -> Result<(), ProtocolError>;
}
/// Dropped before the audio worker so final native exports still have a live owner.
/// Snapshot IPC has a deadline; filesystem I/O and a concurrent control mutation
/// can still delay joining. This owner never abandons an in-flight file replacement.
/// This is never constructed or destroyed on the real-time audio thread.
pub struct ControlMaintenance {
    observing: Arc<AtomicBool>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<Result<(), ProtocolError>>>,
    // Deliberately retain the controller through the final shutdown checkpoint.
    _controller: Arc<dyn MaintenanceTarget>,
}

impl ControlMaintenance {
    /// Reclaims retired recording sessions and checkpoints closed native editors off-RT.
    /// # Errors
    /// Returns a thread creation error without starting a detached observer.
    pub fn start(controller: Arc<dyn MaintenanceTarget>) -> Result<ControlMaintenance, String> {
        let (stop, receive) = mpsc::channel();
        let weak = Arc::downgrade(&controller);
        let observing = Arc::new(AtomicBool::new(true));
        let worker_observing = observing.clone();
        let thread = thread::Builder::new()
            .name("mixrelay-control-maintenance".into())
            .spawn(move || {
                let mut ledger = CloseLedger::default();
                while matches!(
                    receive.recv_timeout(Duration::from_millis(500)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    if !worker_observing.load(Ordering::Acquire) {
                        continue;
                    }
                    let Some(controller) = weak.upgrade() else {
                        return Ok(());
                    };
                    controller.maintain_control(&mut ledger);
                }
                weak.upgrade()
                    .map_or(Ok(()), |controller| controller.finalize_control())
            })
            .map_err(|error| format!("control maintenance failed to start: {error}"))?;
        Ok(ControlMaintenance {
            observing,
            stop: Some(stop),
            thread: Some(thread),
            _controller: controller,
        })
    }
}
impl ControlMaintenance {
    /// Stops admitting periodic restore/checkpoint work without abandoning an
    /// in-flight operation. Final persistence begins only when `finish` is called.
    pub fn stop_observing(&self) {
        self.observing.store(false, Ordering::Release);
    }

    /// Explicit shutdown reports persistence failure to the service caller.
    /// # Errors
    /// Returns a failed final checkpoint or an observer panic after joining the thread.
    pub fn finish(mut self) -> Result<(), String> {
        self.join().inspect_err(|error| {
            tracing::error!(error, "control maintenance shutdown failed");
        })
    }

    fn join(&mut self) -> Result<(), String> {
        self.stop_observing();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.thread.take().map_or(Ok(()), |thread| {
            crate::worker_join::join(thread)?
                .map_err(|error| format!("control shutdown finalization failed: {error:?}"))
        })
    }
}

impl Drop for ControlMaintenance {
    fn drop(&mut self) {
        if let Err(error) = self.join() {
            tracing::error!(error, "control maintenance shutdown failed");
        }
    }
}
