//! Canonical product format choices shared by every backend.
//!
//! These are product policy, not proof of native hardware/backend support.
//! Native build boundaries import this module to generate their C declarations;
//! there is no separately maintained JSON or native catalog.
pub const SAMPLE_RATES: &[u32] = &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
pub const BUFFER_SIZES: &[u32] = &[32, 64, 128, 256, 512, 1024, 2048];
pub const MAX_BUFFER_FRAMES: usize = BUFFER_SIZES[BUFFER_SIZES.len() - 1] as usize;
/// File-recorder channel ceiling; independent of hardware and virtual-device limits.
pub const MAX_RECORDING_CHANNELS: u16 = 8;
