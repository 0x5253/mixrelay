//! Connects the native latency observer to the Driver control transport.
use mixrelay_driver_adapters::DriverClient;
use mixrelay_driver_core::DriverError;
use mixrelay_driver_core::{DriverSnapshot, LatencyCommand, LatencyReply};
use mixrelay_macos_engine::{LatencyPublication, PublicationError, PublicationFuture};

pub(super) struct DriverLatencyPublication(DriverClient);

impl DriverLatencyPublication {
    pub(super) fn new() -> Result<Self, DriverError> {
        DriverClient::new().map(Self)
    }
}

impl LatencyPublication for DriverLatencyPublication {
    fn snapshot(&self) -> PublicationFuture<'_, DriverSnapshot> {
        Box::pin(async move {
            self.0
                .snapshot()
                .await
                .map_err(|source| Box::new(source) as PublicationError)
        })
    }

    fn publish_latency<'a>(
        &'a self,
        request: &'a LatencyCommand,
    ) -> PublicationFuture<'a, LatencyReply> {
        Box::pin(async move {
            self.0
                .publish_latency(request)
                .await
                .map_err(|source| Box::new(source) as PublicationError)
        })
    }
}
