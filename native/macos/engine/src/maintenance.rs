//! Owned native format observation and off-audio-thread reclamation.
use crate::MacDevices;
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use mixrelay_render::ProcessorExchange;
use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Stops, wakes and joins a maintenance worker on the control thread. It must
/// never be moved into its own worker or dropped from an audio callback.
pub struct MaintenanceWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MaintenanceWorker {
    /// Cancels and joins the observer, retaining any panic as a shutdown error.
    /// # Errors
    /// Returns the worker identity and original panic reason.
    pub fn finish(mut self) -> Result<(), String> {
        self.join()
    }

    fn join(&mut self) -> Result<(), String> {
        self.request_stop();
        self.thread
            .take()
            .map_or(Ok(()), mixrelay_engine_host::join_worker)
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    pub(crate) fn start(
        name: &'static str,
        work: impl FnOnce(&AtomicBool) + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || work(&worker_stop))
            .map_err(|error| format!("{name} failed to start: {error}"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for MaintenanceWorker {
    fn drop(&mut self) {
        // Cancellation/unwind still joins; explicit finish also propagates failure.
        let _ = self.join();
    }
}

/// Reclaims retired processors/devices outside the audio thread. Keep this guard
/// alive until the audio worker joins. Weak references do not extend engine life.
/// # Errors
/// Returns an error if the worker cannot be spawned.
pub fn start_reclaimer(
    exchange: Weak<ProcessorExchange>,
    devices: Weak<MacDevices>,
) -> Result<MaintenanceWorker, String> {
    MaintenanceWorker::start("mixrelay-processor-reclaimer", move |stop| {
        while !stop.load(Ordering::Acquire) {
            if !reclaim_once(&exchange, &devices) {
                break;
            }
            thread::park_timeout(Duration::from_millis(100));
        }
    })
}

// Upgraded owners cannot escape one pass into the worker's wait interval.
// false means the engine owner is gone, so there is nothing left to supervise.
fn reclaim_once(exchange: &Weak<ProcessorExchange>, devices: &Weak<MacDevices>) -> bool {
    let Some(exchange) = exchange.upgrade() else {
        return false;
    };
    exchange.reclaim_retired();
    if let Some(devices) = devices.upgrade() {
        devices.reclaim();
    }
    true
}

/// Observes device clocks and reconciles graphs off the audio thread. Drop this
/// guard before finishing control persistence.
/// # Errors
/// Returns a thread-start failure without publishing an observer.
pub fn start_format_monitor(
    status: &Arc<AdapterRuntimeStatus>,
    devices: &Arc<MacDevices>,
    endpoint: &Arc<crate::MacVirtualEndpointBackend>,
    reconcile: impl Fn() -> Option<Result<(), mixrelay_control_protocol::ProtocolError>>
    + Send
    + 'static,
) -> Result<MaintenanceWorker, String> {
    let weak_status = Arc::downgrade(status);
    let weak_devices = Arc::downgrade(devices);
    let weak_endpoint = Arc::downgrade(endpoint);
    MaintenanceWorker::start("mixrelay-device-format-monitor", move |stop| {
        let mut last_error = None;
        let mut last_trace = Instant::now();
        while !stop.load(Ordering::Acquire) {
            let (Some(status), Some(endpoint)) = (weak_status.upgrade(), weak_endpoint.upgrade())
            else {
                break;
            };
            if endpoint
                .snapshot()
                .is_ok_and(|snapshot| snapshot.state == mixrelay_driver_core::DriverState::Ready)
            {
                status.observe_endpoint(true);
            } else {
                status.forget_endpoint_observation();
            }
            if let Some(devices) = weak_devices.upgrade() {
                if last_trace.elapsed() >= Duration::from_secs(1) {
                    devices.trace_audio();
                    last_trace = Instant::now();
                }
                devices.recover(stop);
                if stop.load(Ordering::Acquire) {
                    break;
                }
                status.observe_routing_clock(devices.follows_hardware(), devices.routing_rate());
            }
            // Errors retain the observed mismatch and retry, never write the old rate.
            let Some(observed) = reconcile() else {
                break;
            };
            let error = observed.err();
            if error != last_error {
                if let Some(error) = &error {
                    tracing::error!(?error, "MixRelay external rate reconciliation failed");
                }
                last_error = error;
            }
            drop(status);
            drop(endpoint);
            thread::park_timeout(Duration::from_millis(if last_error.is_some() {
                1000
            } else {
                250
            }));
        }
    })
}

#[cfg(test)]
mod tests;
