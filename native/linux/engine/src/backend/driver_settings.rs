//! Virtual device settings read from and written to the independent Driver service.
use super::{LinuxVirtualEndpointBackend, backend_failure};
use mixrelay_audio_io::{
    HostAudioBackendError, NativeStreamFormat, VirtualEndpointBackend, VirtualEndpointCapabilities,
    VirtualEndpointDeviceSettings, VirtualEndpointId, VirtualEndpointSettings, VirtualEndpointSpec,
};
use mixrelay_driver_core::{DriverCommand, DriverSnapshot, DriverState, NativeDirection};

impl LinuxVirtualEndpointBackend {
    /// The Driver's snapshot, accepted only while the Driver reports ready.
    pub(crate) fn driver_snapshot(&self) -> Result<DriverSnapshot, HostAudioBackendError> {
        let snapshot = self.driver.snapshot().map_err(driver_failure)?;
        if snapshot.state != DriverState::Ready {
            return Err(driver_failure("Driver service is not ready"));
        }
        Ok(snapshot)
    }
}

impl VirtualEndpointBackend for LinuxVirtualEndpointBackend {
    fn settings(&self) -> Option<VirtualEndpointSettings> {
        self.driver_snapshot()
            .ok()
            .map(|snapshot| snapshot.settings)
    }

    fn sample_rate(&self, id: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        self.driver_snapshot()?
            .settings
            .devices
            .into_iter()
            .find(|device| &device.spec.id == id)
            .and_then(|device| device.observed_sample_rate)
            .ok_or_else(|| driver_failure("Driver device rate unavailable"))
    }

    fn set_sample_rate(
        &self,
        id: &VirtualEndpointId,
        rate: u32,
    ) -> Result<(), HostAudioBackendError> {
        let snapshot = self.driver_snapshot()?;
        let device = snapshot
            .settings
            .devices
            .iter()
            .find(|device| &device.spec.id == id)
            .ok_or_else(|| driver_failure("Driver device is missing"))?;
        let (direction, format) = rate_change(device, rate)?;
        self.driver
            .mutate(&DriverCommand::SetFormat {
                expected_revision: snapshot.revision,
                endpoint_id: id.clone(),
                direction,
                format,
            })
            .map_err(driver_failure)?;
        Ok(())
    }

    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        &self.capabilities
    }

    fn prepare_graph_attachments(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.resolve_graph_devices(desired).map(|_| ())
    }

    fn resolve_graph_devices(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<Vec<VirtualEndpointSpec>, HostAudioBackendError> {
        if desired.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.driver_snapshot()?;
        desired
            .iter()
            .map(|reference| {
                snapshot
                    .settings
                    .devices
                    .iter()
                    .find(|device| device.spec.id == reference.id)
                    .map(|device| device.spec.clone())
                    .ok_or_else(|| {
                        driver_failure(format!(
                            "Driver device is missing: {}",
                            reference.id.as_str()
                        ))
                    })
            })
            .collect()
    }
}

/// The Driver's clock is shared by both native directions, so one revision-checked
/// mutation of the first observed direction moves their rate together. Each direction
/// keeps its PCM representation, and both must support the requested rate.
fn rate_change(
    device: &VirtualEndpointDeviceSettings,
    rate: u32,
) -> Result<(NativeDirection, NativeStreamFormat), HostAudioBackendError> {
    let mut selection = None;
    for (direction, formats) in [
        (NativeDirection::Render, &device.native_formats.render),
        (NativeDirection::Capture, &device.native_formats.capture),
    ] {
        let Some(current) = formats.current else {
            continue;
        };
        let desired = NativeStreamFormat {
            sample_rate: rate,
            ..current
        };
        if !formats.supported.contains(&desired) {
            return Err(driver_failure(
                "Driver does not support the requested native format",
            ));
        }
        selection.get_or_insert((direction, desired));
    }
    selection.ok_or_else(|| driver_failure("Driver format unavailable"))
}

fn driver_failure(error: impl std::fmt::Display) -> HostAudioBackendError {
    backend_failure(error.to_string())
}

#[cfg(test)]
mod tests;
