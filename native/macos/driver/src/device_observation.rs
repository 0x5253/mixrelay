//! The device properties and installation observation consumed by Driver orchestration.
use crate::endpoint_registry::EndpointId;
use mixrelay_audio_io::{NativeDeviceFormats, NativeStreamFormat};
use mixrelay_driver_core::{DriverDeviceConfig, DriverError};

pub(crate) trait DeviceProperties: Send + Sync {
    fn inventory(&self) -> Result<Vec<DriverDeviceConfig>, DriverError>;
    fn present(&self, id: EndpointId) -> bool;
    fn rate(&self, id: EndpointId) -> Option<u32>;
    fn supports(&self, id: EndpointId, rate: u32) -> bool;
    fn formats(&self, id: EndpointId) -> NativeDeviceFormats;
    fn set_rate(&self, id: EndpointId, rate: u32) -> bool;
    fn set_format(&self, id: EndpointId, render: bool, format: NativeStreamFormat) -> bool;
}

pub(crate) trait HalInstallation: Send + Sync {
    fn installed(&self) -> bool;
}
