//! Independent Driver configuration and latency policy with injected runtime inputs.

mod contract;
mod latency;
mod lifecycle;
mod manager;
mod runtime;
pub use latency::{
    EngineLatencyLease, LATENCY_ROUTE, LatencyCommand, LatencyReply, RuntimeLatency,
};
mod settings;
mod transport_compatibility;
pub use transport_compatibility::parse_transport_compatibility;

#[cfg(any(test, feature = "test-util"))]
pub mod test_backend;
#[cfg(any(test, feature = "test-util"))]
pub mod test_runtime;
#[cfg(any(test, feature = "test-util"))]
pub mod test_settings;

pub use contract::{
    COMMAND_ROUTE, DriverBackend, DriverCommand, DriverDeviceConfig, DriverError, DriverSnapshot,
    DriverState, JSON_LIMIT, NativeDirection, SNAPSHOT_ROUTE, TransportFailure,
};
pub use lifecycle::{DriverRequests, set_active};
pub use manager::{DriverMaintenance, DriverManager};
pub use runtime::{DriverProcessId, DriverRuntime, LatencyGenerations};
pub use settings::{SettingsStore, StoredSettings};
