//! Endpoint geometry published before client queues exist.

/// Immutable device geometry published independently of client queues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointGeometry {
    pub id: [u8; 16],
    pub render_channels: u32,
    pub capture_channels: u32,
}
impl EndpointGeometry {
    pub(super) fn valid(self) -> bool {
        self.id != [0; 16]
            && self.render_channels <= 64
            && self.capture_channels <= 64
            && self.render_channels + self.capture_channels > 0
    }
}
