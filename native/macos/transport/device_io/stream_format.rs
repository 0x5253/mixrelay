//! Device stream admission, independent of tap packet geometry.

use mixrelay_pcm_ring::SampleFormat;

use crate::device_buffers::{BufferSelection, NativeFormats};

/// Scalar facts copied from a native stream description. No PCM or object ownership.
#[derive(Debug, Clone, Copy)]
pub(super) struct StreamFormat {
    /// Actual native frames per second; never a requested replacement rate.
    pub rate: f64,
    /// Native format identifier.
    pub format: u32,
    /// Native format flags.
    pub flags: u32,
    /// Significant bits in a sample.
    pub bits: u32,
    /// Channels in this stream.
    pub channels: u32,
    /// Bytes in an interleaved frame or one planar channel frame.
    pub frame_bytes: u32,
}

/// Accumulator used by one synchronous native format query.
/// Failed admission leaves the previously accepted prefix unchanged.
#[derive(Debug, Default)]
pub(super) struct StreamFormats {
    channels: u32,
    rate: f64,
    layout: NativeFormats,
}

impl StreamFormats {
    /// Admits rate, byte geometry and codec within `limit`, preserving the prefix on rejection.
    pub(crate) fn accept(&mut self, format: &StreamFormat, limit: u32) -> bool {
        let Some(pcm) = codec(format) else {
            return false;
        };
        if !matches!(limit, 64 | 2048)
            || !format.rate.is_finite()
            || !(8000.0..=384_000.0).contains(&format.rate)
            || format.rate.fract() != 0.0
            || !(1..=limit).contains(&format.channels)
        {
            return false;
        }
        let frame_channels = if format.flags & 32 != 0 {
            1
        } else {
            format.channels
        };
        let Some(channels) = self.channels.checked_add(format.channels) else {
            return false;
        };
        let Ok(sample_bytes) = u32::try_from(pcm.bytes_per_sample()) else {
            return false;
        };
        if format.frame_bytes != sample_bytes * frame_channels
            || (self.rate != 0.0 && self.rate.to_bits() != format.rate.to_bits())
            || channels > limit
        {
            return false;
        }
        if !self
            .layout
            .append(format.channels, format.flags & 32 != 0, pcm)
        {
            return false;
        }
        self.channels = channels;
        self.rate = format.rate;
        true
    }

    /// Moves the fixed buffer mapping into the drained callback owner.
    pub(crate) fn into_layout(self) -> NativeFormats {
        self.layout
    }

    pub(crate) fn selected_rate(&self, selection: BufferSelection) -> Option<f64> {
        let end = selection.offset.checked_add(selection.channels)?;
        (matches!(selection.limit, 64 | 2048)
            && (1..=64).contains(&selection.channels)
            && self.rate != 0.0
            && end <= self.channels
            && self.channels <= selection.limit
            && (selection.total == 0 || selection.total == self.channels))
            .then_some(self.rate)
    }
}

fn codec(format: &StreamFormat) -> Option<SampleFormat> {
    // The shared converters consume little-endian, low-aligned, packed integer PCM.
    // Planar and nonmixable flags change geometry/policy, not sample representation.
    if format.format != u32::from_be_bytes(*b"lpcm") || format.flags & !(1 | 4 | 8 | 32 | 64) != 0 {
        return None;
    }
    match (format.flags & (1 | 4 | 8), format.bits) {
        (1 | 9, 32) => Some(SampleFormat::Float32),
        (12, 16) => Some(SampleFormat::Signed16),
        (12, 24) => Some(SampleFormat::Signed24),
        (12, 32) => Some(SampleFormat::Signed32),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
