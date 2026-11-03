//! Control-side native observation and off-audio-thread reclamation. Both workers
//! have an explicit owner; shutdown waits for their current operation to finish.

use mixrelay_engine::session::AdapterRuntimeStatus;
use mixrelay_engine_runtime::control_maintenance::ControlMaintenance;
use mixrelay_macos_engine::{
    MacDevices, MacVirtualEndpointBackend, MaintenanceWorker, start_format_monitor,
};
use mixrelay_service::EngineGraphController;
use std::sync::{Arc, Weak};

/// Field order also governs cancellation/unwind: stop native observation before
/// the final control checkpoint, while the caller still owns its audio worker.
pub(super) struct ControlObservers {
    format_monitor: Option<MaintenanceWorker>,
    control: Option<ControlMaintenance>,
    devices: Weak<MacDevices>,
}

impl ControlObservers {
    pub(super) fn start(
        status: &Arc<AdapterRuntimeStatus>,
        controller: &Arc<EngineGraphController>,
        devices: &Arc<MacDevices>,
        endpoint: &Arc<MacVirtualEndpointBackend>,
    ) -> Result<Self, String> {
        // If monitor startup fails, control maintenance still finalizes and joins.
        let mut observers = Self {
            format_monitor: None,
            control: Some(controller.start_control_maintenance()?),
            devices: Arc::downgrade(devices),
        };
        let controller = Arc::downgrade(controller);
        observers.format_monitor = Some(start_format_monitor(
            status,
            devices,
            endpoint,
            move || {
                controller
                    .upgrade()
                    .map(|controller| controller.reconcile_observed_device_rate().map(|_| ()))
            },
        )?);
        Ok(observers)
    }

    pub(super) fn stop_observing(&self) {
        if let Some(devices) = self.devices.upgrade() {
            devices.stop_preparing();
        }
        if let Some(control) = &self.control {
            control.stop_observing();
        }
        if let Some(monitor) = &self.format_monitor {
            monitor.request_stop();
        }
    }

    pub(super) fn finish(mut self) -> Result<(), String> {
        self.stop_observing();
        let observed = self
            .format_monitor
            .take()
            .map_or(Ok(()), MaintenanceWorker::finish);
        // Persistence must still finish if the native observer panicked.
        let saved = self
            .control
            .take()
            .map_or(Ok(()), ControlMaintenance::finish);
        observed.and(saved)
    }
}

impl Drop for ControlObservers {
    fn drop(&mut self) {
        // Cancel all observation before either join, including startup unwind.
        self.stop_observing();
    }
}

#[cfg(test)]
mod tests;
