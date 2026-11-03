//! Fallible off-realtime PCM storage for the bridge FIFO and conversion scratch.
use super::BridgeError;

pub(super) fn zeroed(samples: usize) -> Result<Vec<f32>, BridgeError> {
    crate::pcm_buffer::zeroed(samples).map_err(|_| BridgeError::Allocation { samples })
}
