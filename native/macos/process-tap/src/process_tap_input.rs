//! Validated tap PCM delivery and independently drained channel peaks.

use std::sync::atomic::{AtomicU32, Ordering};

mod ingest;
pub(crate) use ingest::{ingest, input_frames};

const MAX_CHANNELS: usize = 64;
const FLOAT: u32 = 1;
const BIG_ENDIAN: u32 = 2;
const PACKED: u32 = 8;
const PLANAR: u32 = 32;

/// Scalar projection of a `CoreAudio` stream description. No native object ownership.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TapFormat {
    /// Negotiated frames per second.
    pub(crate) sample_rate: f64,
    /// `CoreAudio` format identifier.
    pub(crate) format_id: u32,
    /// `CoreAudio` PCM encoding flags.
    pub(crate) flags: u32,
    /// Bytes in one packet.
    pub(crate) bytes_per_packet: u32,
    /// Frames in one packet.
    pub(crate) frames_per_packet: u32,
    /// Bytes in one interleaved frame or one planar channel frame.
    pub(crate) bytes_per_frame: u32,
    /// Total channel count across the callback buffers.
    pub(crate) channels: u32,
    /// Significant bits per channel sample.
    pub(crate) bits: u32,
}

impl TapFormat {
    pub(crate) fn supported(&self) -> bool {
        self.format_id == u32::from_be_bytes(*b"lpcm")
            && self.flags & (FLOAT | PACKED | BIG_ENDIAN) == FLOAT | PACKED
            && self.bits == 32
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && (1..=64).contains(&self.channels)
            && self.bytes_per_frame == 4 * if self.planar() { 1 } else { self.channels }
            && self.frames_per_packet == 1
            && self.bytes_per_packet == self.bytes_per_frame
    }

    fn planar(&self) -> bool {
        self.flags & PLANAR != 0
    }

    pub(crate) fn same_geometry(&self, other: &Self) -> bool {
        self.format_id == other.format_id
            && self.flags == other.flags
            && self.bytes_per_packet == other.bytes_per_packet
            && self.frames_per_packet == other.frames_per_packet
            && self.bytes_per_frame == other.bytes_per_frame
            && self.channels == other.channels
            && self.bits == other.bits
    }
}

/// Embedded atomic meter storage. Zero-initialized native memory is valid; its
/// session owner retains it through callback, meter-reader and refresh drain.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct TapPeaks {
    values: [AtomicU32; MAX_CHANNELS],
}

impl Default for TapPeaks {
    fn default() -> Self {
        Self {
            values: [const { AtomicU32::new(0) }; MAX_CHANNELS],
        }
    }
}

/// A sample's contribution to a level meter; non-finite samples are faults, not levels.
fn finite_magnitude(sample: f32) -> f32 {
    let magnitude = sample.abs();
    if magnitude.is_finite() {
        magnitude
    } else {
        0.0
    }
}

impl TapPeaks {
    /// Publishes one callback block's per-channel peaks, once per channel rather than
    /// per sample. A drain racing the publication may count some channels' block peaks
    /// in its next window, but no peak is lost. Positive finite IEEE bits have numeric order.
    fn publish(&self, block: &[f32]) {
        for (value, &peak) in self.values.iter().zip(block) {
            let proposed = peak.to_bits();
            if value.load(Ordering::Relaxed) < proposed {
                value.fetch_max(proposed, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn clear(&self) {
        for value in &self.values {
            value.store(0, Ordering::Relaxed);
        }
    }

    pub(crate) fn take(&self, destination: &mut [f32]) {
        for (source, output) in self.values.iter().zip(destination) {
            *output = f32::from_bits(source.swap(0, Ordering::AcqRel));
        }
    }
}

/// Frame observations from one validated callback; rejected ring writes still
/// contribute valid input frames to clock continuity and metering.
#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TapInputResult {
    /// Complete frames presented by the native buffers, or zero for malformed input.
    pub(crate) frames: usize,
    /// Complete frames accepted by the capture queue.
    pub(crate) written: usize,
    /// At least one finite nonzero sample was observed, independently of ring capacity.
    pub(crate) audible: bool,
}

const _: () = assert!(size_of::<TapFormat>() == 40 && align_of::<TapFormat>() == 8);
const _: () = assert!(size_of::<TapPeaks>() == 256 && align_of::<TapPeaks>() == 4);
const _: () = assert!(size_of::<TapInputResult>() == 24 && align_of::<TapInputResult>() == 8);

#[cfg(test)]
mod tests;
