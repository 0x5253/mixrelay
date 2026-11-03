//! Framing contract shared by the editor and its background runtime.
use std::time::Duration;

/// One multiplexed connection carries commands and telemetry with this wire version.
pub const LINK_PROTOCOL: u64 = 3;
/// Bound each command before allocating or decoding its complete JSON frame.
pub const FRAME_LIMIT: u64 = 4 * 1024 * 1024;
/// Bound each reply before allocating or decoding its complete JSON frame.
pub const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
/// The engine publishes every 50 ms; two seconds without a window ends the stream.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(2);
