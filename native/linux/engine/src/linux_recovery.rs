//! Control-thread preparation and bounded publication of replacement native owners.
use super::{DeviceStreams, LinuxVirtualEndpointBackend};
use mixrelay_render::ProcessorExchange;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub struct DeviceRecoveryWorker {
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl DeviceRecoveryWorker {
    /// Stops and joins the worker; a panic means the service did not stop cleanly.
    /// # Errors
    /// Returns an error if the recovery worker panicked while retiring.
    pub fn finish(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        self.stopping.store(true, Ordering::Release);
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "MixRelay device recovery worker panicked".to_owned())
        })
    }
}
impl Drop for DeviceRecoveryWorker {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_join() {
            tracing::error!(%error);
        }
    }
}

/// Starts the native resource recovery worker.
/// # Errors
/// Returns thread creation failure before publishing a recovery owner.
pub fn start(
    backend: &Arc<LinuxVirtualEndpointBackend>,
    exchange: &Arc<ProcessorExchange>,
) -> Result<DeviceRecoveryWorker, String> {
    let weak = Arc::downgrade(backend);
    let weak_exchange = Arc::downgrade(exchange);
    let stopping = Arc::new(AtomicBool::new(false));
    let stop = stopping.clone();
    let worker = thread::Builder::new()
        .name("mixrelay-device-recovery".into())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let Some(backend) = weak.upgrade() else {
                    break;
                };
                backend.device_recovery.reclaim();
                backend.recover_applications(&stop);
                if let Some(exchange) = weak_exchange.upgrade() {
                    backend.poll_source_mute(&exchange);
                }
                if backend.device_recovery.idle() {
                    // Do not hold the configuration mutex across discovery/open.
                    let watch = backend
                        .device_watch
                        .lock()
                        .ok()
                        .and_then(|watch| watch.clone());
                    if let Some(watch) = watch
                        && watch.needs_observation()
                        && let Ok(mut prepared) = DeviceStreams::recover(&watch, &stop)
                        && !prepared.is_empty()
                        && !stop.load(Ordering::Acquire)
                        && backend.bind_device_inputs(&mut prepared).is_ok()
                    {
                        // A concurrent edit may have replaced/deleted these nodes.
                        // The audio epoch and per-stream lease checks reject stale results.
                        let _ = backend.device_recovery.publish(watch.generation, prepared);
                    }
                }
                drop(backend);
                thread::sleep(Duration::from_millis(500));
            }
        })
        .map_err(|error| format!("could not start device recovery: {error}"))?;
    Ok(DeviceRecoveryWorker {
        stopping,
        worker: Some(worker),
    })
}
