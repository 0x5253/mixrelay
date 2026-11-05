//! Control-side worker that reports Driver readiness and drives format reconciliation.
use crate::runtime::EndpointDriver;
use mixrelay_driver_core::DriverState;
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub struct FormatMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FormatMonitor {
    pub fn stop_observing(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }

    /// Polls the supplied Driver and domain callback off the audio thread.
    /// # Errors
    /// Returns the native thread creation failure without leaving a worker running.
    pub fn start(
        status: &Arc<AdapterRuntimeStatus>,
        driver: Arc<dyn EndpointDriver>,
        mut reconcile: impl FnMut() -> bool + Send + 'static,
    ) -> Result<Self, String> {
        let status = Arc::downgrade(status);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("mixrelay-pipewire-format-monitor".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    let Some(status) = status.upgrade() else {
                        break;
                    };
                    observe_driver(
                        &status,
                        driver.snapshot().ok().map(|snapshot| snapshot.state),
                    );
                    if !reconcile() {
                        break;
                    }
                    drop(status);
                    thread::park_timeout(Duration::from_millis(250));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    /// Stops and joins the monitor; a panic means the service did not stop cleanly.
    /// # Errors
    /// Returns an error if the native observation worker panicked.
    pub fn finish(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        self.stop_observing();
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "MixRelay endpoint format monitor panicked".to_owned())
        })
    }
}

impl Drop for FormatMonitor {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_join() {
            tracing::error!(%error);
        }
    }
}

/// An unreachable Driver is reported as unobserved rather than as a stale state.
fn observe_driver(status: &AdapterRuntimeStatus, observed: Option<DriverState>) {
    match observed {
        Some(state) => status.observe_driver_state(state),
        _ => status.forget_endpoint_observation(),
    }
}

#[cfg(test)]
mod tests;
