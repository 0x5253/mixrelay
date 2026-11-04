//! Supported render formats and the callback-owned evidence of the negotiated one.
use crate::endpoints::EndpointSet;
use mixrelay_audio_graph::RenderFormat;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

mod monitor;

pub(crate) use mixrelay_audio_io::formats::{BUFFER_SIZES, SAMPLE_RATES};
pub use monitor::FormatMonitor;

/// Callback-owned endpoint rate and quantum evidence for one native backend.
#[derive(Debug, Default)]
pub struct EndpointObservation {
    rate: AtomicU32,
    // Internal callback diagnostics; this is not a user-selected fixed buffer size.
    quantum: AtomicU64,
}

impl EndpointObservation {
    /// Returns a rate only when all active endpoint streams agree.
    #[must_use]
    pub fn rate(&self) -> Option<u32> {
        let rate = self.rate.load(Ordering::Acquire);
        (rate != 0).then_some(rate)
    }

    pub(crate) fn observe(&self, endpoints: &EndpointSet) {
        let mut agreed = None;
        let mut quantum = 0;
        for stream in endpoints.streams() {
            let observed = stream.callback_quantum();
            if observed != 0 {
                quantum = observed;
            }
            let rate = (stream.negotiated_format() >> 32) as u32;
            if rate == 0 || agreed.is_some_and(|previous| previous != rate) {
                self.rate.store(0, Ordering::Release);
                self.quantum.store(0, Ordering::Release);
                return;
            }
            agreed = Some(rate);
        }
        // Only currently active native owners contribute; staged streams do not.
        self.rate.store(agreed.unwrap_or(0), Ordering::Release);
        self.quantum.store(quantum, Ordering::Release);
    }

    /// The latest callback quantum as `(rate, frames)` packed by the native stream.
    #[cfg(all(test, feature = "native-lab"))]
    pub(crate) fn quantum(&self) -> u64 {
        self.quantum.load(Ordering::Acquire)
    }
}

/// Packs a render format into one atomic word.
pub(crate) const fn pack(format: RenderFormat) -> u64 {
    (format.sample_rate as u64) << 32 | format.block_frames as u64
}

pub(crate) const fn unpack(value: u64) -> RenderFormat {
    let [h0, h1, h2, h3, l0, l1, l2, l3] = value.to_be_bytes();
    RenderFormat {
        sample_rate: u32::from_be_bytes([h0, h1, h2, h3]),
        block_frames: u32::from_be_bytes([l0, l1, l2, l3]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_format_round_trips_every_supported_combination() {
        for &sample_rate in SAMPLE_RATES {
            for &block_frames in BUFFER_SIZES {
                let format = RenderFormat {
                    sample_rate,
                    block_frames,
                };
                assert_eq!(unpack(pack(format)), format);
            }
        }
    }
}
