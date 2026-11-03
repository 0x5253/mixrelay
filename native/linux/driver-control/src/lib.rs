//! Independent per-user virtual-device ownership and native-clock audio return path.
//! Inventory decisions use prepared-owner ports; native owners drain on Drop.

#[cfg(any(target_os = "linux", test))]
mod admission;
#[cfg(target_os = "linux")]
mod device;
#[cfg(target_os = "linux")]
mod loopback;
#[cfg(any(target_os = "linux", test))]
mod plan;
#[cfg(any(target_os = "linux", test))]
mod reconcile;
#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(target_os = "linux")]
use mixrelay_audio_io::{VirtualEndpointId, VirtualEndpointSettings};
#[cfg(target_os = "linux")]
use mixrelay_driver_control::{DriverBackend, DriverDeviceConfig, DriverError, DriverState};
#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};

/// The installed user-session service's unique native device owner.
/// Drop drains callbacks before releasing the native control connection.
#[cfg(target_os = "linux")]
pub struct LinuxDriver {
    inventory: reconcile::Inventory<device::NativeDevices>,
    _connection: mixrelay_pipewire::Connection,
}

/// Stable canonical node-name root, retaining the existing native device identity.
#[cfg(target_os = "linux")]
#[must_use]
pub fn native_name(id: &VirtualEndpointId) -> String {
    format!(
        "io.mixrelay.endpoint.{:x}",
        Sha256::digest(id.as_str().as_bytes())
    )
}

/// Published directional identities, never graph-node IDs or implicit defaults.
#[cfg(target_os = "linux")]
#[must_use]
pub fn native_names(spec: &mixrelay_audio_io::VirtualEndpointSpec) -> Vec<String> {
    let name = native_name(&spec.id);
    spec.render
        .iter()
        .map(|_| format!("{name}.render"))
        .chain(spec.capture.iter().map(|_| format!("{name}.capture")))
        .collect()
}

#[cfg(target_os = "linux")]
impl LinuxDriver {
    /// Connects to the current user's `PipeWire` session without starting a graph engine.
    /// # Errors
    /// Returns the actual native connection/start failure.
    pub fn open() -> Result<Self, DriverError> {
        Self::open_with_owners(mixrelay_pipewire::StreamOwners::default())
    }

    fn open_with_owners(owners: mixrelay_pipewire::StreamOwners) -> Result<Self, DriverError> {
        let mut connection =
            mixrelay_pipewire::Connection::new(c"MixRelay Driver").map_err(backend)?;
        connection.start().map_err(backend)?;
        Ok(Self {
            inventory: reconcile::Inventory::new(device::NativeDevices::new(
                owners,
                std::sync::Arc::new(mixrelay_time::StdClock::new()),
            )),
            _connection: connection,
        })
    }
}

#[cfg(target_os = "linux")]
impl DriverBackend for LinuxDriver {
    fn settings(&self) -> Result<VirtualEndpointSettings, DriverError> {
        self.inventory.settings()
    }
    fn replace(&mut self, desired: &[DriverDeviceConfig]) -> Result<(), DriverError> {
        self.inventory.replace(desired).map_err(reconcile_error)
    }
    fn recover(&mut self, desired: &[DriverDeviceConfig]) -> Result<bool, DriverError> {
        self.inventory.recover(desired).map_err(reconcile_error)
    }
    fn state(&self) -> DriverState {
        self.inventory.state()
    }
}

#[cfg(target_os = "linux")]
fn reconcile_error(error: reconcile::ReconcileError<DriverError>) -> DriverError {
    match error {
        reconcile::ReconcileError::Duplicate => DriverError::Invalid(error.to_string()),
        reconcile::ReconcileError::Invalid(source) => DriverError::Invalid(source.to_string()),
        reconcile::ReconcileError::Native(source) => source,
        reconcile::ReconcileError::Rollback { .. } => DriverError::Unavailable(error.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn backend(error: impl std::fmt::Display) -> DriverError {
    DriverError::Backend(error.to_string())
}
