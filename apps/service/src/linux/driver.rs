//! Driver transport selected by the Linux service composition root.
use mixrelay_driver_adapters::DriverClient;
use mixrelay_driver_control::{DriverCommand, DriverError, DriverSnapshot};
use mixrelay_linux_engine::EndpointDriver;

pub(super) struct DriverEndpointControl(Result<DriverClient, DriverError>);
impl DriverEndpointControl {
    pub(super) fn new() -> Self {
        Self(DriverClient::new())
    }
    fn client(&self) -> Result<&DriverClient, DriverError> {
        self.0
            .as_ref()
            .map_err(|error| DriverError::Backend(error.to_string()))
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
