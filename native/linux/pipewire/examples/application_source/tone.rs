/// Exact supported source rates keep phase conversion within f32's integer precision.
#[derive(Clone, Copy, Debug)]
pub(super) enum Rate {
    /// 44,100 frames per second.
    Hz44100,
    /// 48,000 frames per second.
    Hz48000,
}

impl Rate {
    /// Native sample rate, bounded below 2^16 for exact phase conversion.
    pub(super) const fn hz(self) -> u16 {
        match self {
            Self::Hz44100 => 44_100,
            Self::Hz48000 => 48_000,
        }
    }
}

/// Left is a continuous 1 kHz tone; right carries exact frame identity for no-SRC checks.
pub(super) struct Tone {
    // One exact second of the 1 kHz waveform, built before process callbacks
    // are registered so the real-time path never calls libm's sine function.
    cycle: Box<[f32]>,
    phase: usize,
    position: u64,
}

impl Tone {
    /// Builds a one-second tone table off-callback and starts at frame zero.
    pub(super) fn new(rate: Rate) -> Self {
        let cycle = (0..rate.hz())
            .map(|phase| {
                0.125
                    * (2.0 * std::f32::consts::PI * 1000.0 * f32::from(phase)
                        / f32::from(rate.hz()))
                    .sin()
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            cycle,
            phase: 0,
            position: 0,
        }
    }

    /// Number of completely generated stereo frames, independent of native ticks.
    pub(super) const fn position(&self) -> u64 {
        self.position
    }

    /// Fills only whole requested frames, leaving the unused buffer suffix untouched.
    /// A zero request means the available capacity, matching native stream semantics.
    pub(super) fn fill(&mut self, output: &mut [u8], requested: u64) -> u32 {
        let capacity = output.len() / 8;
        let frames = if requested == 0 {
            capacity
        } else {
            usize::try_from(requested)
                .unwrap_or(usize::MAX)
                .min(capacity)
        };
        // Native maxsize is u32 bytes. Reject an impossible non-native slice
        // without writing rather than narrowing its resulting chunk length.
        let Some(frames) = u32::try_from(frames)
            .ok()
            .filter(|frames| *frames <= u32::MAX / 8)
        else {
            return 0;
        };
        for frame in output.chunks_exact_mut(8).take(frames as usize) {
            let value = self.cycle[self.phase];
            frame[..4].copy_from_slice(&value.to_ne_bytes());
            frame[4..].copy_from_slice(
                &mixrelay_test_support::frame_sequence::encode(self.position).to_ne_bytes(),
            );
            self.position += 1;
            self.phase += 1;
            if self.phase == self.cycle.len() {
                self.phase = 0;
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sample_rates_produce_one_thousand_cycles_without_zero_runs() {
        for rate in [Rate::Hz44100, Rate::Hz48000] {
            let mut bytes = vec![0; usize::from(rate.hz()) * 8];
            assert_eq!(Tone::new(rate).fill(&mut bytes, 0), u32::from(rate.hz()));
            let mut previous = 0.0;
            let mut crossings = 0;
            for (index, frame) in bytes.chunks_exact(8).enumerate() {
                let sample = f32::from_ne_bytes(frame[..4].try_into().unwrap());
                assert!(index == 0 || sample != 0.0 || previous != 0.0);
                if previous <= 0.0 && sample > 0.0 {
                    crossings += 1;
                }
                previous = sample;
            }
            assert_eq!(crossings, 1000);
        }
    }

    #[test]
    fn lookup_cycle_preserves_exact_tone_bits_and_sequence_tags_across_wraps() {
        for rate in [Rate::Hz44100, Rate::Hz48000] {
            let cycle_len = usize::from(rate.hz());
            let frame_count = cycle_len * 2 + 7;
            let mut bytes = vec![0; frame_count * 8];
            let mut tone = Tone::new(rate);
            assert_eq!(
                tone.fill(&mut bytes, u64::try_from(frame_count).unwrap()),
                u32::try_from(frame_count).unwrap()
            );

            for (index, frame) in bytes.chunks_exact(8).enumerate() {
                let phase = u16::try_from(index % cycle_len).unwrap();
                let expected = 0.125
                    * (2.0 * std::f32::consts::PI * 1000.0 * f32::from(phase)
                        / f32::from(rate.hz()))
                    .sin();
                assert_eq!(
                    &frame[..4],
                    &expected.to_ne_bytes(),
                    "tone sample differs at frame {index} for {rate:?}"
                );
                assert_eq!(
                    &frame[4..],
                    &mixrelay_test_support::frame_sequence::encode(u64::try_from(index).unwrap())
                        .to_ne_bytes(),
                    "frame tag differs at frame {index} for {rate:?}"
                );
            }
            assert_eq!(tone.position(), u64::try_from(frame_count).unwrap());
        }
    }

    #[test]
    fn requests_bound_writes_and_split_buffers_preserve_the_same_phase() {
        for rate in [Rate::Hz44100, Rate::Hz48000] {
            let mut whole = [0_u8; 384];
            let mut split = [0_u8; 384];
            assert_eq!(Tone::new(rate).fill(&mut whole, 0), 48);
            let mut source = Tone::new(rate);
            assert_eq!(source.fill(&mut split[..80], u64::MAX), 10);
            assert_eq!(source.fill(&mut split[80..], 38), 38);
            assert_eq!(source.position(), 48);
            assert_eq!(whole, split);
            for (index, frame) in whole.chunks_exact(8).enumerate() {
                let left = f32::from_ne_bytes(frame[..4].try_into().unwrap());
                let right = f32::from_ne_bytes(frame[4..].try_into().unwrap());
                assert!(left.is_finite() && left.abs() <= 0.125);
                assert_eq!(
                    mixrelay_test_support::frame_sequence::decode(right),
                    u32::try_from(index).ok(),
                );
            }
            let mut partial = [0xa5; 27];
            assert_eq!(source.fill(&mut partial, 2), 2);
            assert_eq!(&partial[16..], &[0xa5; 11]);
        }
    }
}
