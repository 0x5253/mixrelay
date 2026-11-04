//! Control-thread preparation and bounded publication of replacement native owners.
use super::LinuxVirtualEndpointBackend;
use crate::{devices::DeviceStreams, source_mute::PlaybackResources};
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
pub fn start_recovery(
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
                backend.recover_devices(&stop);
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

impl LinuxVirtualEndpointBackend {
    fn recover_applications(&self, stopping: &AtomicBool) {
        self.application_recovery.reclaim();
        if !self.application_recovery.idle() {
            return;
        }
        let watch = self
            .application_watch
            .lock()
            .ok()
            .and_then(|watch| watch.clone());
        let Some(watch) = watch else {
            return;
        };
        let Ok(Some((mut prepared, next_watch))) = watch.recover(stopping) else {
            return;
        };
        if stopping.load(Ordering::Acquire) {
            return;
        }
        if self.bind_application_inputs(&mut prepared).is_err() {
            return;
        }
        if let Ok(mut registered) = self.application_watch.lock()
            && registered
                .as_ref()
                .is_some_and(|current| current.generation == watch.generation)
            && self
                .application_recovery
                .publish(watch.generation, prepared)
                .is_ok()
            && let Some(current) = registered.as_mut()
        {
            current.refresh_leases(&next_watch);
        }
    }

    /// Reconciles source-mute leases with the plan the audio thread runs.
    pub(crate) fn poll_source_mute(&self, exchange: &ProcessorExchange) {
        let watch = self
            .application_watch
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let Some(spec) = self.active_spec.lock().ok().map(|value| value.clone()) else {
            return;
        };
        let driver = if watch.is_some() && !spec.endpoints.is_empty() {
            self.driver_snapshot().ok()
        } else {
            None
        };
        self.playback.poll(
            exchange,
            &PlaybackResources {
                watch: watch.as_ref(),
                endpoints: &spec.endpoints,
                driver: driver.as_ref(),
            },
        );
    }

    fn recover_devices(&self, stopping: &AtomicBool) {
        if !self.device_recovery.idle() {
            return;
        }
        // Do not hold the configuration mutex across discovery/open.
        let watch = self
            .device_watch
            .lock()
            .ok()
            .and_then(|watch| watch.clone());
        if let Some(watch) = watch
            && watch.needs_observation()
            && let Ok(mut prepared) = DeviceStreams::recover(&watch, stopping)
            && !prepared.is_empty()
            && !stopping.load(Ordering::Acquire)
            && self.bind_device_inputs(&mut prepared).is_ok()
        {
            // A concurrent edit may have replaced/deleted these nodes.
            // The audio epoch and per-stream lease checks reject stale results.
            let _ = self.device_recovery.publish(watch.generation, prepared);
        }
    }
}
