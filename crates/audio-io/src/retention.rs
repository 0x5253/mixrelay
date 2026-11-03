//! Preallocated backlog storage, independent of callback size and latency targets.

/// Preserve four seconds at every supported rate, including a live rate change.
pub const SECONDS: u32 = 4;
/// Allocate at the highest product rate before audio admission; never resize on RT.
pub const FRAMES: u32 =
    crate::formats::SAMPLE_RATES[crate::formats::SAMPLE_RATES.len() - 1] * SECONDS;
/// Timestamped transports use a power-of-two ring and round the retention up.
pub const RING_FRAMES: u32 = FRAMES.next_power_of_two();
