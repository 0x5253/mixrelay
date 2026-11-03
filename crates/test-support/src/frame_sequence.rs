//! Exact frame identity for unconverted f32 test routes, never audible product PCM.
use serde::Serialize;

const MASK: u32 = (1 << 23) - 1;
const PREFIX: u32 = 0x3d00_0000;
const EVENT_LIMIT: usize = 64;

/// Encodes the low 23 position bits as a finite sample in `[0.03125, 0.0625)`.
/// Consecutive frames remain distinguishable, including a whole sine-period loss.
/// Identity wraps after 2^23 frames; this does not establish absolute latency or
/// detect a loss of that entire period. SRC, gain and integer PCM invalidate the
/// bit-preserving contract; use waveform checks separately for those paths.
#[must_use]
pub const fn encode(position: u64) -> f32 {
    // Explicit low bytes keep the bounded conversion infallible on every host.
    let [low, middle, high, ..] = position.to_le_bytes();
    f32::from_bits(PREFIX | u32::from_le_bytes([low, middle, high & 0x7f, 0]))
}

/// Returns the frame tag only for the exact marker exponent/sign representation.
#[must_use]
pub fn decode(sample: f32) -> Option<u32> {
    let bits = sample.to_bits();
    (bits & !MASK == PREFIX).then_some(bits & MASK)
}

/// One observed break; indices refer to captured samples, not native callback time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SequenceBreak {
    /// Zero-based sample position in the analyzed capture.
    pub sample_index: usize,
    /// Expected next tag; absent before the first valid marker.
    pub expected: Option<u32>,
    /// Received tag; absent for silence, nonfinite or a wrong representation.
    pub actual: Option<u32>,
}

/// Frame-order evidence for a bit-preserving route, measured off the RT thread.
/// Silence/corruption never resets the previous valid tag or disappears from totals.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SequenceContinuity {
    /// Every inspected sample, including invalid samples.
    pub sample_count: usize,
    /// Samples outside the exact marker representation.
    pub invalid_samples: usize,
    /// Unexpected transitions between valid tags, including duplicates/backward jumps.
    pub discontinuities: usize,
    /// First and last valid tags, modulo 2^23 frames.
    pub first_frame: Option<u32>,
    /// Final valid tag, modulo 2^23 frames.
    pub last_frame: Option<u32>,
    /// First 64 invalid samples/transitions, with exact captured positions.
    pub breaks: Vec<SequenceBreak>,
    /// Further events omitted from the bounded detail list, not from failure totals.
    pub omitted_breaks: usize,
}

impl SequenceContinuity {
    /// Inspects one channel in time order without requiring a particular start tag.
    /// No decoding tolerance or sample filtering is applied. Storage is bounded to
    /// 64 break records; this allocation belongs on the diagnostic control thread.
    #[must_use]
    pub fn measure(samples: impl IntoIterator<Item = f32>) -> Self {
        let mut report = Self::default();
        for sample in samples {
            let actual = decode(sample);
            let expected = report.last_frame.map(|previous| (previous + 1) & MASK);
            let broken = if let Some(frame) = actual {
                report.first_frame.get_or_insert(frame);
                report.last_frame = Some(frame);
                let unexpected = expected.is_some_and(|expected| frame != expected);
                report.discontinuities += usize::from(unexpected);
                unexpected
            } else {
                report.invalid_samples += 1;
                true
            };
            if broken {
                if report.breaks.len() < EVENT_LIMIT {
                    report.breaks.push(SequenceBreak {
                        sample_index: report.sample_count,
                        expected,
                        actual,
                    });
                } else {
                    report.omitted_breaks += 1;
                }
            }
            report.sample_count += 1;
        }
        report
    }

    /// At least two tagged frames, with no invalid sample or order discontinuity.
    #[must_use]
    pub const fn intact(&self) -> bool {
        self.sample_count >= 2 && self.invalid_samples == 0 && self.discontinuities == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_representation_and_wrap_preserve_consecutive_identity() {
        for frame in [0, 1, 48, u64::from(MASK), u64::from(MASK) + 1, u64::MAX] {
            let sample = encode(frame);
            assert!(sample.is_finite() && (0.03125..0.0625).contains(&sample));
            assert_eq!(decode(sample), u32::try_from(frame & u64::from(MASK)).ok());
        }
        let begin = u64::from(MASK) - 256;
        assert!(SequenceContinuity::measure((begin..begin + 1024).map(encode)).intact());
        for sample in [0.0, -0.0, 0.125, f32::NAN, f32::INFINITY, encode(42) * 0.5] {
            assert_eq!(decode(sample), None);
        }
        assert!(!SequenceContinuity::measure([]).intact());
        assert!(!SequenceContinuity::measure([encode(8)]).intact());
    }

    #[test]
    fn detects_full_tone_period_deletion_duplication_and_reordering() {
        let frames: Vec<f32> = (0..1024).map(encode).collect();
        let measure = |values: &[f32]| SequenceContinuity::measure(values.iter().copied());
        assert!(measure(&frames).intact());
        let mut deleted = frames.clone();
        deleted.drain(128..176); // A full 1 kHz period at 48 kHz.
        let report = measure(&deleted);
        assert!(!report.intact());
        assert_eq!(
            report.breaks,
            [SequenceBreak {
                sample_index: 128,
                expected: Some(128),
                actual: Some(176),
            }]
        );
        let mut duplicated = frames.clone();
        duplicated.splice(128..128, frames[80..128].iter().copied());
        assert!(!measure(&duplicated).intact());
        let mut reordered = frames.clone();
        reordered.swap(127, 128);
        assert_eq!(measure(&reordered).discontinuities, 3);
    }

    #[test]
    fn invalid_frames_are_counted_without_hiding_the_following_transition() {
        let frames = [encode(10), 0.0, f32::NAN, encode(13), encode(14)];
        let report = SequenceContinuity::measure(frames.chunks(2).flatten().copied());
        assert_eq!(report.sample_count, 5);
        assert_eq!(report.invalid_samples, 2);
        assert_eq!(report.discontinuities, 1);
        assert_eq!(report.breaks[2].sample_index, 3);
        assert_eq!(report.breaks[2].expected, Some(11));
        assert_eq!(report.breaks[2].actual, Some(13));
        let corrupt = SequenceContinuity::measure([0.0; 1000]);
        assert_eq!(corrupt.invalid_samples, 1000);
        assert_eq!(corrupt.breaks.len(), 64);
        assert_eq!(corrupt.omitted_breaks, 936);
        assert!(!corrupt.intact());
    }
}
