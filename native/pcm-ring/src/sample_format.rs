/// Interleaved little-endian native PCM representations. Packed 24-bit uses three bytes.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// IEEE 754 single precision; all bits, including NaNs, are preserved.
    Float32 = 1,
    /// Signed 16-bit PCM normalized by 2^15.
    Signed16 = 2,
    /// Packed signed 24-bit PCM normalized by 2^23.
    Signed24 = 3,
    /// Signed 32-bit PCM normalized by 2^31.
    Signed32 = 4,
}

impl SampleFormat {
    /// Bytes occupied by one sample, independent of channel count.
    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::Signed16 => 2,
            Self::Signed24 => 3,
            Self::Float32 | Self::Signed32 => 4,
        }
    }

    /// Decodes exactly `output.len()` samples without allocation. Returns false
    /// on a byte-length mismatch without modifying output.
    #[must_use]
    pub fn decode(self, input: &[u8], output: &mut [f32]) -> bool {
        if output.len().checked_mul(self.bytes_per_sample()) != Some(input.len()) {
            return false;
        }
        for (bytes, sample) in input.chunks_exact(self.bytes_per_sample()).zip(output) {
            *sample = self.decode_sample(bytes);
        }
        true
    }

    /// Encodes exactly `input.len()` samples without allocation. Returns false
    /// on a byte-length mismatch without modifying output. Integer encoding
    /// silences NaNs, saturates infinities/overs and truncates toward zero.
    #[must_use]
    pub fn encode(self, input: &[f32], output: &mut [u8]) -> bool {
        if input.len().checked_mul(self.bytes_per_sample()) != Some(output.len()) {
            return false;
        }
        for (sample, bytes) in input
            .iter()
            .zip(output.chunks_exact_mut(self.bytes_per_sample()))
        {
            self.encode_sample(*sample, bytes);
        }
        true
    }

    pub(super) const fn from_raw(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Float32),
            2 => Some(Self::Signed16),
            3 => Some(Self::Signed24),
            4 => Some(Self::Signed32),
            _ => None,
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "native signed PCM is intentionally rounded to f32"
    )]
    fn decode_sample(self, bytes: &[u8]) -> f32 {
        match self {
            Self::Float32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            Self::Signed16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0,
            Self::Signed24 => {
                let sign = if bytes[2] & 0x80 == 0 { 0 } else { 0xff };
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign]) as f32 / 8_388_608.0
            }
            Self::Signed32 => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
        }
    }

    fn encode_sample(self, sample: f32, bytes: &mut [u8]) {
        let encoded = match self {
            Self::Float32 => sample.to_le_bytes(),
            Self::Signed16 => quantize(sample, 32768.0).to_le_bytes(),
            Self::Signed24 => quantize(sample, 8_388_608.0).to_le_bytes(),
            Self::Signed32 => quantize(sample, 2_147_483_648.0).to_le_bytes(),
        };
        bytes.copy_from_slice(&encoded[..self.bytes_per_sample()]);
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded signed PCM quantization truncates toward zero"
)]
fn quantize(sample: f32, scale: f64) -> i32 {
    if sample.is_nan() {
        return 0;
    }
    (f64::from(sample) * scale).clamp(-scale, scale - 1.0) as i32
}
