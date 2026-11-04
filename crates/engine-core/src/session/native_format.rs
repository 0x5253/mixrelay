//! How an adapter's session format may change; session policy decides when.

use std::sync::Arc;

type DeviceRateReader = Arc<dyn Fn() -> Option<u32> + Send + Sync>;

/// Adapters reconfigure their own graph format and may follow an endpoint's nominal
/// rate. They never write a device's nominal format through the session.
#[derive(Default)]
pub(super) struct NativeFormat {
    graph_control: bool,
    read_rate: Option<DeviceRateReader>,
}

impl NativeFormat {
    pub(super) fn allow_graph_control(&mut self) {
        self.graph_control = true;
    }
    pub(super) fn follow_rate(&mut self, reader: DeviceRateReader) {
        self.read_rate = Some(reader);
    }
    pub(super) fn follows_rate(&self) -> bool {
        self.read_rate.is_some()
    }
    pub(super) fn can_reconfigure(&self) -> bool {
        self.graph_control
    }
    pub(super) fn owns_format(&self) -> bool {
        self.can_reconfigure() || self.follows_rate()
    }
    pub(super) fn read_rate(&self) -> Option<u32> {
        self.read_rate.as_ref().and_then(|reader| reader())
    }
}
