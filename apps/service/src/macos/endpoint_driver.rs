//! Defers the endpoint adapter's Driver transport until a graph actually references it.
use mixrelay_driver_adapters::DriverClient;
use mixrelay_driver_control::{DriverCommand, DriverError, DriverSnapshot};
use mixrelay_macos_engine::EndpointDriver;
use parking_lot::Mutex;

#[derive(Default)]
pub(super) struct DriverEndpointControl(Mutex<Option<DriverClient>>);

impl DriverEndpointControl {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn client(&self) -> Result<DriverClient, DriverError> {
        let mut client = self.0.lock();
        if let Some(client) = client.as_ref() {
            return Ok(client.clone());
        }
        let prepared = DriverClient::new()?;
        *client = Some(prepared.clone());
        Ok(prepared)
    }
}

impl EndpointDriver for DriverEndpointControl {
    fn snapshot(&self) -> Result<DriverSnapshot, DriverError> {
        self.client()?.snapshot_blocking()
    }

    fn mutate(&self, command: &DriverCommand) -> Result<DriverSnapshot, DriverError> {
        self.client()?.mutate_blocking(command)
    }
}
